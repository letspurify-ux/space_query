# MySQL 접속 및 테스트 정리

## 로컬 MySQL 8 접속 정보

- Container: `space-query-mysql80`
- Image: `mysql:8.0`
- Server version: `8.0.46`
- Host: `127.0.0.1`
- Port: `3307`
- Database: `query_tool_mysql8`
- Username: `root`
- Password: `spacequery`
- 앱 DB 타입: `MySQL or MariaDB` (`DatabaseType::MySQL`)

클라이언트 접속 확인:

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3307 -uroot -pspacequery query_tool_mysql8
```

버전 확인:

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3307 -uroot -pspacequery -e "SELECT VERSION();"
```

## 테스트 환경 변수

```sh
export SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1
export SPACE_QUERY_TEST_MYSQL_PORT=3307
export SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_mysql8
export SPACE_QUERY_TEST_MYSQL_USER=root
export SPACE_QUERY_TEST_MYSQL_PASSWORD=spacequery
```

한 번에 실행할 때:

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3307 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_mysql8 \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD=spacequery \
  cargo test mysql_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
```

## 고급 옵션 적용 테스트

메인 연결의 고급 옵션 적용 확인:

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3307 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_mysql8 \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD=spacequery \
  cargo test mysql_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
```

쿼리 실행에서 사용하는 풀 세션의 고급 옵션 적용 확인:

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3307 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_mysql8 \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD=spacequery \
  cargo test mysql_pool_session_applies_advanced_session_settings --lib -- --ignored --nocapture
```

위 테스트들은 다음 값을 실제 세션에서 조회해 검증한다.

- `@@SESSION.sql_mode`
- `@@SESSION.time_zone`
- `@@SESSION.character_set_client`
- `@@SESSION.collation_connection`
- `@@transaction_isolation`

## 앱에서 적용하는 고급 옵션

접속 직후와 풀 세션 획득 시 다음 설정을 적용한다.

- `SET SESSION sql_mode = ...`
- `SET SESSION time_zone = ...`
- `SET SESSION TRANSACTION ISOLATION LEVEL ...`
- `SET NAMES <charset> [COLLATE <collation>]`

`default_transaction_access_mode`는 접속 직후 SQL로 바로 고정하지 않는다. 앱의 현재 transaction mode로 저장한 뒤 실제 쿼리 실행 직전에 `SET SESSION TRANSACTION READ WRITE` 또는 `READ ONLY` 형태로 적용한다.

## Session Time Zone 범위

MySQL 8 로컬 서버에서 확인한 offset 범위:

- 허용: `-13:59`부터 `+14:00`
- 거부: `-14:00`, `+14:01`, `+14:59`

직접 확인:

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3307 -uroot -pspacequery \
  -e "SET SESSION time_zone = '+14:00'; SELECT @@SESSION.time_zone;"
```

앱은 `MySQL or MariaDB` 공통 backend를 사용한다. 따라서 MySQL에서는 `+14:00` 같은 MySQL 전용 offset도 허용하되, MariaDB에서만 거부되는 범위는 실제 서버 버전 확인 후 더 명확한 오류를 반환한다.

## 문자셋과 Collation 주의사항

앱은 `mysql_charset`과 `mysql_collation`을 검증한 뒤 `SET NAMES`를 실행한다. 서로 맞지 않는 조합은 접속 시점에서 막는다.

허용되는 예:

- `utf8mb4` + `utf8mb4_unicode_ci`
- `utf8` + `utf8mb3_general_ci`
- `utf8mb3` + `utf8_general_ci`
- `binary` + `binary`

거부되는 예:

- `utf8mb4` + `latin1_swedish_ci`

직접 확인:

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3307 -uroot -pspacequery \
  -e "SET NAMES utf8 COLLATE utf8mb3_general_ci; SELECT @@character_set_client, @@collation_connection;"
```

```sh
mysql --protocol=TCP -h 127.0.0.1 -P 3307 -uroot -pspacequery \
  -e "SET NAMES binary COLLATE binary; SELECT @@character_set_client, @@collation_connection;"
```

## 테스트 중 확인한 문제

### 메인 연결만 확인하면 부족함

앱은 접속 확인과 실제 쿼리 실행에서 서로 다른 세션을 사용할 수 있다. 접속 직후 메인 연결에는 옵션이 적용되더라도, 풀에서 꺼낸 세션에 같은 옵션이 적용되지 않으면 실제 실행 결과가 달라질 수 있다.

해결:

- `DbConnectionPool::acquire_session()`에서 MySQL 세션을 얻을 때 `apply_mysql_session_settings()`를 다시 호출한다.
- `mysql_pool_session_applies_advanced_session_settings` 테스트로 풀 세션에도 옵션이 적용되는지 검증한다.

### Collation 검증 예외

단순 prefix 비교만 사용하면 실제 MySQL이 허용하는 `utf8`/`utf8mb3` 별칭 조합과 `binary`/`binary` 조합을 잘못 거부할 수 있다.

해결:

- `utf8`과 `utf8mb3` collation prefix를 상호 허용한다.
- `binary` charset의 `binary` collation도 허용한다.
- 관련 단위 테스트:
  - `mysql_advanced_validation_accepts_utf8_utf8mb3_alias_collations`
  - `mysql_advanced_validation_accepts_binary_charset_collation`
  - `mysql_set_names_statement_accepts_utf8_utf8mb3_alias_collations`
  - `mysql_set_names_statement_accepts_binary_database_collation`

### Session Time Zone 경계값

기존 검증은 형식과 `14:59` 이하 여부만 확인해서 MySQL 8이 거부하는 `+14:59`가 접속 시점까지 통과할 수 있었다.

해결:

- MySQL 범위를 `-13:59`부터 `+14:00`까지로 제한했다.
- Oracle은 별도 범위를 유지한다.
- MariaDB에서만 좁은 범위는 실제 서버가 MariaDB일 때 별도 오류로 안내한다.
