# MariaDB 접속 및 테스트 정리

> 구현 및 로컬 환경 대조: 2026-07-12

## 앱에서의 처리 방식

현재 앱은 MariaDB를 `DatabaseType::MariaDB`로 구분한다. 실행 구현은 MySQL 계열 backend를 공유하지만, timeout provider, 세션 time zone 검증, `SET STATEMENT ... FOR` 분류처럼 MariaDB 전용 차이가 필요한 지점에서는 구체 DB 타입을 유지한다.

적용되는 고급 옵션은 MySQL/MariaDB 공통 세션 SQL로 실행된다.

- `SET SESSION sql_mode = ...`
- `SET SESSION time_zone = ...`
- `SET SESSION TRANSACTION ISOLATION LEVEL ...`
- `SET NAMES <charset> [COLLATE <collation>]`

메인 연결뿐 아니라 쿼리 실행에서 사용하는 풀 세션을 얻을 때도 같은 세션 설정을 다시 적용한다.

## 로컬 MariaDB 접속값

현재 로컬 MariaDB 테스트 DB 접속 정보:

- Container: `space-query-mariadb122`
- Image: `mariadb:12.2.2`
- Server version: `12.2.2-MariaDB-ubu2404`
- Host: `127.0.0.1`
- Port: `3306`
- Database: `query_tool_test`
- User: `root`
- Password: `password`
- 앱 DB 타입: `MariaDB` (`DatabaseType::MariaDB`)

클라이언트 접속 확인:

```sh
mariadb -h 127.0.0.1 -P 3306 -uroot -p'password' -e "SELECT VERSION();"
```

## 테스트 환경 변수

MariaDB ignored 통합 테스트는 다음 환경 변수를 사용한다.

```sh
export SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1
export SPACE_QUERY_TEST_MYSQL_PORT=3306
export SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_test
export SPACE_QUERY_TEST_MYSQL_USER=root
export SPACE_QUERY_TEST_MYSQL_PASSWORD='password'
```

한 번에 실행할 때는 `env`로 지정한다.

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3306 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_test \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD='password' \
  cargo test mariadb_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
```

## 고급 옵션 적용 테스트

메인 연결의 고급 옵션 적용 확인:

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3306 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_test \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD='password' \
  cargo test mariadb_connect_applies_advanced_session_settings --lib -- --ignored --nocapture
```

쿼리 실행에서 사용하는 풀 세션의 고급 옵션 적용 확인:

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3306 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_test \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD='password' \
  cargo test mariadb_pool_session_applies_advanced_session_settings --lib -- --ignored --nocapture
```

기본 풀 세션 설정 확인:

```sh
env \
  SPACE_QUERY_TEST_MYSQL_HOST=127.0.0.1 \
  SPACE_QUERY_TEST_MYSQL_PORT=3306 \
  SPACE_QUERY_TEST_MYSQL_DATABASE=query_tool_test \
  SPACE_QUERY_TEST_MYSQL_USER=root \
  SPACE_QUERY_TEST_MYSQL_PASSWORD='password' \
  cargo test mysql_pool_session_applies_default_session_settings_from_local_mariadb --lib -- --ignored --nocapture
```

위 테스트들은 다음 값을 실제 세션에서 조회해 검증한다.

- `@@SESSION.sql_mode`
- `@@SESSION.time_zone`
- `@@SESSION.character_set_client`
- `@@SESSION.collation_connection`
- `@@transaction_isolation` 또는 `@@tx_isolation`

## Session Time Zone 범위

MariaDB 12.2.2 로컬 서버에서 확인한 offset 범위:

- 허용: `-12:59`부터 `+13:00`
- 거부: `-13:00`, `+13:01`, `+14:00`, `+14:59`

직접 확인:

```sh
mariadb -h 127.0.0.1 -P 3306 -uroot -p'password' \
  -e "SET SESSION time_zone = '+13:00'; SELECT @@SESSION.time_zone;"
```

앱은 `DatabaseType::MariaDB`의 backend metadata에서 이 범위를 저장 전에
검증한다. 연결 후에도 서버 버전이 MariaDB인지 확인하는 방어 검증을 거쳐,
범위를 벗어난 값은 `SET SESSION time_zone` 실행 전에 명확한 오류로 반환한다.

## 문자셋과 Collation 주의사항

앱은 `mysql_charset`과 `mysql_collation`을 검증한 뒤 `SET NAMES`를 실행한다. 서로 맞지 않는 조합은 접속 시점에서 막는다.

예:

- 허용: `utf8mb4` + `utf8mb4_unicode_ci`
- 거부: `utf8mb4` + `latin1_swedish_ci`

MariaDB/MySQL은 `utf8`과 `utf8mb3`를 서로 별칭처럼 허용한다. 앱 검증도 이 조합을 허용한다.

허용되는 예:

- `utf8` + `utf8mb3_general_ci`
- `utf8mb3` + `utf8_general_ci`
- `binary` + `binary`

직접 확인:

```sh
mariadb -h 127.0.0.1 -P 3306 -uroot -p'password' \
  -e "SET NAMES utf8 COLLATE utf8mb3_general_ci; SELECT @@character_set_client, @@collation_connection;"
```

```sh
mariadb -h 127.0.0.1 -P 3306 -uroot -p'password' \
  -e "SET NAMES binary COLLATE binary; SELECT @@character_set_client, @@collation_connection;"
```

## 테스트 중 확인한 문제

### 메인 연결만 확인하면 부족함

앱은 접속 확인과 실제 쿼리 실행에서 서로 다른 세션을 사용할 수 있다. 접속 직후 메인 연결에는 옵션이 적용되더라도, 풀에서 꺼낸 세션에 같은 옵션이 적용되지 않으면 실제 실행 결과가 달라질 수 있다.

해결:

- `DbConnectionPool::acquire_session()`에서 MySQL/MariaDB 세션을 얻을 때 `apply_mysql_session_settings()`를 다시 호출한다.
- `mariadb_pool_session_applies_advanced_session_settings` 테스트로 풀 세션에도 옵션이 적용되는지 검증한다.

### Collation 검증이 너무 엄격할 수 있음

단순 prefix 비교만 사용하면 실제 MariaDB/MySQL이 허용하는 `utf8`/`utf8mb3` 별칭 조합을 잘못 거부할 수 있다.

해결:

- `utf8`과 `utf8mb3` collation prefix를 상호 허용하도록 검증을 조정했다.
- `binary` charset의 `binary` collation도 허용하도록 조정했다.
- 관련 단위 테스트:
  - `mysql_advanced_validation_accepts_utf8_utf8mb3_alias_collations`
  - `mysql_advanced_validation_accepts_binary_charset_collation`
  - `mysql_set_names_statement_accepts_utf8_utf8mb3_alias_collations`
  - `mysql_set_names_statement_accepts_binary_database_collation`

### MySQL과 MariaDB에서 동일 테스트 필요

MariaDB는 MySQL backend를 공유하지만 서버 동작이 완전히 같다고 가정하면 안 된다. 특히 SQL mode, isolation 변수명, charset/collation 처리에서 버전 차이가 있을 수 있다.

검증 기준:

- MariaDB `3306`에서 메인 연결과 풀 세션 고급 옵션 테스트를 실행한다.
- MySQL 8 `3307`에서도 같은 테스트를 실행해 공통 backend 변경이 양쪽 모두에서 동작하는지 확인한다.

### Session Time Zone 범위가 MySQL과 다름

MySQL 8은 `+14:00`, `-13:59`를 허용하지만 MariaDB 12.2.2는 각각 `+13:00`, `-12:59`까지만 허용했다.

해결:

- MariaDB backend의 저장 전 검증부터 MariaDB 범위를 적용한다.
- 서버 버전 기반 방어 검증에서도 범위를 벗어나면 `MariaDB session time zone ... is outside MariaDB's supported offset range` 오류를 반환한다.
