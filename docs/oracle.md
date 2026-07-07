# Oracle Instant Client 접속 및 테스트 정리

## 설치된 클라이언트

- 다운로드 출처: Oracle Instant Client Downloads for macOS ARM64
  - https://www.oracle.com/database/technologies/instant-client/macos-arm64-downloads.html
- Codex/sandbox 테스트용 설치 경로:
  - `/tmp/oqt_instantclient_23_26`
- 다운로드 파일:
  - `/tmp/instantclient-basic-macos.arm64-23.26.1.0.0.dmg`
  - SHA256: `5dc67a7e1cccd0a01d5bf53d7cf13b56f00999e3c2c1a309d8600cd766d80b41`
- 설치 패키지:
  - Basic Package `23.26.1.0.0`
  - SQL*Plus Package는 `/tmp/oqt_instantclient_23_26`에 기본 설치되어 있지 않다. 필요하면 같은 버전 SQL*Plus DMG를 추가 설치한다.

설치 확인:

```sh
file /tmp/oqt_instantclient_23_26/libclntsh.dylib.23.1
/tmp/oqt_instantclient_23_26/genezi -v
```

정상 결과:

- `libclntsh.dylib.23.1`이 `Mach-O 64-bit ... arm64`로 표시된다.
- `genezi -v`가 `Client Shared Library 64-bit - 23.26.1.0.0`를 표시한다.

재설치가 필요하면 다음 순서로 `/tmp` 안에 설치한다.

```sh
curl -L -o /tmp/instantclient-basic-macos.arm64-23.26.1.0.0.dmg \
  https://download.oracle.com/otn_software/mac/instantclient/2326100/instantclient-basic-macos.arm64-23.26.1.0.0.dmg
shasum -a 256 /tmp/instantclient-basic-macos.arm64-23.26.1.0.0.dmg
hdiutil attach /tmp/instantclient-basic-macos.arm64-23.26.1.0.0.dmg
mkdir -p /tmp/oqt_instantclient_23_26
cp -R -P /Volumes/instantclient-basic-macos.arm64-23.26.1.0.0/instantclient_23_26/. /tmp/oqt_instantclient_23_26/
```

설치 후 DMG는 필요할 때 분리한다.

```sh
hdiutil detach /Volumes/instantclient-basic-macos.arm64-23.26.1.0.0
```

## 앱 접속 설정

앱은 `oracle-rs`/ODPI-C를 통해 Oracle Client를 초기화한다. macOS에서는 자동 탐색이 다른 아키텍처의 클라이언트를 잡을 수 있으므로, 테스트나 터미널 실행에서는 sandbox 안에 설치한 `/tmp` 경로를 명시한다.

```sh
export ORACLE_CLIENT_LIB_DIR=/tmp/oqt_instantclient_23_26
```

현재 로컬 Docker Oracle 테스트 DB 기준 직접 접속 정보:

- Container: `oracle`
- Image: `gvenzl/oracle-free`
- Server version: `Oracle AI Database 26ai Free Release 23.26.0.0.0`
- Host: `127.0.0.1`
- Port: `1521`
- Service: `FREE`
- SID: `FREE`
- Username: `system`
- Password: `password`
- 앱 DB 타입: `Oracle`

TNS alias 모드에서는 `Host`와 `Port`를 비우고 `Service Name` 자리에 TNS alias를 입력한다. 이 경우 SSL/protocol은 앱의 직접 접속 옵션이 아니라 Oracle Net 설정(`tnsnames.ora`, `sqlnet.ora`)을 따른다.

SQL*Plus 접속 확인은 SQL*Plus 패키지를 추가 설치한 경우에만 실행한다. Basic Package만 설치한 `/tmp/oqt_instantclient_23_26`에는 `sqlplus`가 없다.

```sh
/tmp/oqt_instantclient_23_26/sqlplus -L 'system/password@//127.0.0.1:1521/FREE'
```

Docker listener는 `FREE`, `freepdb1` 서비스가 `READY`로 보여야 한다.

```sh
docker exec oracle bash -lc "lsnrctl status"
```

## 로컬 Docker 테스트 서버 시작

Docker 런타임은 Colima를 사용한다. Docker API 연결이 안 되거나 `colima is not running`이 보이면 먼저 Colima를 시작한다.

```sh
colima start
```

이미 만들어진 기본 테스트 컨테이너는 다음 명령으로 다시 켠다. 컨테이너가 멈춰 있어도 같은 명령을 사용한다.

```sh
docker restart oracle
```

`oracle` 컨테이너가 없는 새 환경에서는 한 번만 생성한다. 초기 생성은 이미지 다운로드와 DB 초기화 때문에 몇 분 걸릴 수 있다.

```sh
docker run -d --name oracle \
  -p 1521:1521 \
  -e ORACLE_PASSWORD=password \
  gvenzl/oracle-free
```

서버가 준비되면 컨테이너 로그에 `DATABASE IS READY TO USE!`가 출력된다. 그 뒤 listener와 접속을 확인한다.

```sh
docker logs --tail 80 oracle
docker exec oracle bash -lc "lsnrctl status"
nc -vz 127.0.0.1 1521
docker exec oracle bash -lc "echo 'select 1 from dual;' | sqlplus -s system/password@//127.0.0.1:1521/FREE"
```

## 선택적 Oracle Net 설정

별도 `TNS_ADMIN`을 사용할 때는 다음 `sqlnet.ora`를 둘 수 있다.

```text
NAMES.DIRECTORY_PATH = (EZCONNECT, TNSNAMES)
DISABLE_OOB=ON
BREAK_POLL_SKIP=1000
```

예시:

```sh
mkdir -p /tmp/oracle_net_admin
printf '%s\n' \
  'NAMES.DIRECTORY_PATH = (EZCONNECT, TNSNAMES)' \
  'DISABLE_OOB=ON' \
  'BREAK_POLL_SKIP=1000' \
  > /tmp/oracle_net_admin/sqlnet.ora

printf '%s\n' \
  'FREE_LOCAL =' \
  '  (DESCRIPTION =' \
  '    (ADDRESS = (PROTOCOL = TCP)(HOST = 127.0.0.1)(PORT = 1521))' \
  '    (CONNECT_DATA =' \
  '      (SERVICE_NAME = FREE)' \
  '    )' \
  '  )' \
  > /tmp/oracle_net_admin/tnsnames.ora
```

## 로컬 live 테스트 실행 가이드

현재 Docker Oracle 기본 listener는 `1521/tcp`만 열려 있다. 그래서 로컬 TCP live 테스트의 성공 기준은 `oracle_tcps_connection_uses_advanced_ssl_protocol`만 제외한 Oracle ignored 테스트 12개 통과다. TCPS listener를 따로 구성한 환경에서만 TCPS 테스트까지 포함해 13개를 실행한다.

다음 블록을 먼저 실행한다. 환경 준비와 사전 점검을 한 번에 끝내고, 조건이 맞지 않으면 테스트 전에 실패한다.

```sh
set -e

export TNS_ADMIN=/tmp/oracle_net_admin
export ORACLE_CLIENT_LIB_DIR=/tmp/oqt_instantclient_23_26
export ORACLE_TEST_USERNAME=system
export ORACLE_TEST_PASSWORD=password
export ORACLE_TEST_HOST=127.0.0.1
export ORACLE_TEST_PORT=1521
export ORACLE_TEST_SERVICE_NAME=FREE
export ORACLE_TEST_SID=FREE
export ORACLE_TEST_TNS_ALIAS=FREE_LOCAL
unset ORACLE_TEST_TARGET

mkdir -p "$TNS_ADMIN"
printf '%s\n' \
  'NAMES.DIRECTORY_PATH = (EZCONNECT, TNSNAMES)' \
  'DISABLE_OOB=ON' \
  'BREAK_POLL_SKIP=1000' \
  > "$TNS_ADMIN/sqlnet.ora"
printf '%s\n' \
  'FREE_LOCAL =' \
  '  (DESCRIPTION =' \
  '    (ADDRESS = (PROTOCOL = TCP)(HOST = 127.0.0.1)(PORT = 1521))' \
  '    (CONNECT_DATA =' \
  '      (SERVICE_NAME = FREE)' \
  '    )' \
  '  )' \
  > "$TNS_ADMIN/tnsnames.ora"

docker inspect -f '{{.State.Running}}' oracle | grep -qx true
docker exec oracle bash -lc "lsnrctl status | grep -q '(PROTOCOL=tcp)(HOST=0.0.0.0)(PORT=1521)'"
docker exec oracle bash -lc "lsnrctl status | grep -q 'Service \"FREE\"'"
docker exec oracle bash -lc "lsnrctl status | grep -q 'Service \"freepdb1\"'"
docker exec oracle bash -lc "lsnrctl status | grep -q 'status READY'"
nc -vz "$ORACLE_TEST_HOST" "$ORACLE_TEST_PORT"
test -r "$ORACLE_CLIENT_LIB_DIR/libclntsh.dylib"
file "$ORACLE_CLIENT_LIB_DIR/libclntsh.dylib"
file "$ORACLE_CLIENT_LIB_DIR/libclntsh.dylib" | grep -q arm64
if [ -n "${ORACLE_TEST_TARGET:-}" ]; then
  rustup target list --installed | grep -qx "$ORACLE_TEST_TARGET"
fi

oracle_cargo_test() {
  if [ -n "${ORACLE_TEST_TARGET:-}" ]; then
    cargo test --target "$ORACLE_TEST_TARGET" "$@"
  else
    cargo test "$@"
  fi
}
```

정상 기준:

- `oracle` 컨테이너가 `Up` 상태다.
- listener endpoint에 `(PROTOCOL=tcp)(HOST=0.0.0.0)(PORT=1521)`가 있다.
- `FREE`, `freepdb1` 서비스가 `READY`다.
- `nc -vz 127.0.0.1 1521`가 성공한다.
- `file "$ORACLE_CLIENT_LIB_DIR/libclntsh.dylib"`가 `arm64`를 표시한다.
- `ORACLE_TEST_TARGET`는 기본적으로 비운다. x86_64 클라이언트를 일부러 쓸 때만 `x86_64-apple-darwin`으로 지정한다.

그 다음 로컬 TCP live 테스트 전체를 실행한다.

```sh
oracle_cargo_test oracle --lib -- --ignored --nocapture --skip oracle_tcps_connection_uses_advanced_ssl_protocol
```

기대 결과:

```text
test result: ok. 12 passed; 0 failed; 0 ignored
```

기존 Homebrew x86_64 Instant Client를 일부러 검증할 때만 다음처럼 바꾼다.

```sh
export ORACLE_CLIENT_LIB_DIR=/opt/homebrew/lib
export ORACLE_TEST_TARGET=x86_64-apple-darwin

file "$ORACLE_CLIENT_LIB_DIR/libclntsh.dylib"
oracle_cargo_test oracle --lib -- --ignored --nocapture --skip oracle_tcps_connection_uses_advanced_ssl_protocol
```

이 경우 `file "$ORACLE_CLIENT_LIB_DIR/libclntsh.dylib"`는 `x86_64`를 표시해야 한다.

TCPS까지 포함하려면 먼저 `2484/tcp` listener가 열려 있어야 한다. 성공할 때만 TCPS 테스트 skip을 제거한다.

```sh
if nc -vz 127.0.0.1 2484; then
  export ORACLE_TEST_TCPS_PORT=2484
  oracle_cargo_test oracle --lib -- --ignored --nocapture
else
  echo 'TCPS listener가 없으므로 기본 TCP 명령의 --skip을 유지한다.'
fi
```

기본 접속 테스트:

```sh
oracle_cargo_test oracle_test_connection_supports_direct_local_xe --lib -- --ignored --nocapture
```

## 고급 옵션 적용 테스트

Oracle ignored 통합 테스트는 로컬 Docker listener와 OCI 네트워크 접속을 사용하므로, Codex에서는 같은 `/tmp` Instant Client 경로를 유지한 채 escalated command로 실행한다.

메인 연결의 고급 옵션 적용 확인:

```sh
oracle_cargo_test oracle_connect_applies_advanced_session_settings_from_local_xe --lib -- --ignored --nocapture
```

쿼리 실행에서 사용하는 풀 세션의 고급 옵션 적용 확인:

```sh
oracle_cargo_test oracle_pool_session_applies_advanced_session_settings_from_local_xe --lib -- --ignored --nocapture
```

위 테스트들은 다음 Oracle 세션 설정이 메인 연결과 풀 세션 모두에 적용되는지 확인한다.

- `ALTER SESSION SET NLS_TIMESTAMP_FORMAT`
- `ALTER SESSION SET NLS_DATE_FORMAT`
- `ALTER SESSION SET ISOLATION_LEVEL`
- `ALTER SESSION SET TIME_ZONE`

## Session Time Zone 범위

Oracle 로컬 서버에서 offset 경계값을 확인했다.

- 허용: `-14:59`, `+14:59`
- 거부 대상으로 앱에서 막는 값: `+15:00` 이상

직접 확인:

```sh
docker exec oracle bash -lc "printf \"ALTER SESSION SET TIME_ZONE = '+14:59';\nSELECT SESSIONTIMEZONE FROM dual;\nEXIT\n\" | sqlplus -s system/password@localhost:1521/FREE"
```

MySQL/MariaDB와 허용 범위가 다르므로 앱 검증도 DB 타입별로 분리한다.

## 테스트 중 확인한 문제

### ARM64 설치 전 DPI-1047

증상:

```text
DPI-1047: Cannot locate a 64-bit Oracle Client library
incompatible architecture (have 'x86_64', need 'arm64')
```

원인:

- ARM64 앱/런타임이 x86_64 Oracle Instant Client를 찾고 있었다.

해결:

- macOS ARM64 Instant Client를 설치한다.
- 자동 탐색이 잘못된 클라이언트를 잡으면 `ORACLE_CLIENT_LIB_DIR`를 ARM64 클라이언트 디렉터리로 지정한다.

### Codex sandbox 내부 ORA-12560

sandbox 안에서 `/tmp/oqt_instantclient_23_26`의 arm64 OCI 라이브러리 로딩은 성공하지만, OCI 네트워크 접속은 다음 오류가 발생할 수 있다.

```text
ORA-12560: Database communication protocol error
```

확인 결과:

- 같은 `/tmp/oqt_instantclient_23_26` 경로를 사용한 OCI child 실행은 sandbox 밖에서 정상 접속된다.
- Docker Oracle listener와 컨테이너 내부 SQL*Plus 접속은 정상이다.
- 따라서 이 문제는 앱 코드나 Instant Client 아키텍처 문제가 아니라, sandbox 안 프로세스가 로컬 Docker listener로 정상 Oracle Net 연결을 만들지 못한 문제였다.

해결 또는 우회:

- `ORACLE_CLIENT_LIB_DIR=/tmp/oqt_instantclient_23_26`는 유지한다.
- Codex에서는 로컬 Docker listener 접속이 필요한 OCI 테스트를 escalated command로 실행한다.

### 호스트명 해석은 최종 원인이 아니었음

macOS 호스트명 `iceblueui-noteubug.local`이 `dscacheutil`에서 해석되지 않고 `/etc/hosts`에도 없어서 원인 후보로 보였다. Oracle client connect data에 로컬 호스트명이 포함되기 때문이다.

하지만 같은 `/tmp/oqt_instantclient_23_26` OCI 경로를 사용한 테스트가 sandbox 밖에서는 `/etc/hosts` 수정 없이 성공했다. 동일 문제가 sandbox 밖에서도 재현될 때만 `/etc/hosts` 수정을 검토한다.

### Session Time Zone 범위가 MySQL/MariaDB와 다름

Oracle은 `+14:59`, `-14:59`를 허용했지만 MySQL/MariaDB는 같은 값을 거부했다. 기존의 공통 형식 검증만으로는 DB별 차이를 반영할 수 없었다.

해결:

- Oracle, MySQL/MariaDB 시간대 offset 검증 범위를 분리했다.
- Oracle은 `-14:59`부터 `+14:59`까지 허용한다.
