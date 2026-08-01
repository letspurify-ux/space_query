# Oracle TNS Thin 변경 기록

## 1. 개요

- 기준 커밋: `dddd38f` (`tns thin 기능 개선`)
- 작성일: 2026-07-18
- 대상: Rust로 직접 구현한 `crates/tns-thin` Oracle Thin 드라이버와 Space Query 통합 경로
- 목표:
  - 실제 사용하는 python-oracledb API 동작과의 호환성 보강
  - TNS 프로토콜 314, 315, 318, 319 지원 검증
  - `test/`의 모든 `.sql`, `.txt` 파일을 OCI와 Thin 양쪽에서 실행해 결과 비교
  - `final.sql`의 모든 결과 그리드와 셀 단위 비교

이번 변경은 13개 파일에 걸쳐 5,942줄 추가, 180줄 삭제로 반영됐다.

## 2. 주요 구현 변경

### 2.1 연결과 인증

- IPv6 호스트의 Easy Connect 문자열에 대괄호를 자동 적용했다.
  - 예: `::1` → `//[::1]:1521/FREEPDB1`
- 서비스명과 SID가 모두 없는 연결 대상은 명시적인 오류를 반환한다.
- 연결 중 비밀번호 변경을 위한 `OracleThinConfig::new_password`와 인증 payload 처리를 추가했다.
  - 비밀번호 변경 후 재연결
  - `/`, `@`가 포함된 비밀번호
  - 비밀번호 길이 경계
  - 잘못된 기존/신규 비밀번호 오류를 검증했다.
- `user[proxy_user]` 형식의 프록시 사용자 연결을 지원하고 세션에서 프록시 정보를 조회할 수 있게 했다.
- 인증 응답에서 다음 서버/세션 메타데이터를 보존하고 공개한다.
  - DB domain/name
  - instance/service name
  - maximum open cursors
  - maximum identifier length
  - server version, username, connect target
- App Context, terminal, program, machine, OS user, edition 등의 인증 메타데이터 전달을 검증했다.
- End User Security Context 타입과 길이 검증을 추가했다. 현재 일반 TCP 연결에서는 TCPS가 필요하다는 명시적 오류를 반환한다.
- 닫힌 세션에서 실행, 커서 바인드 등 네트워크 작업이 계속되지 않도록 공통 open-state 검사를 추가했다.

### 2.2 SQL 파싱과 바인드

- python-oracledb와 같은 규칙으로 SQL 바인드 이름을 추출하는 파서를 추가했다.
  - 일반 문자열과 quoted identifier
  - Oracle `q'...'`, `nq'...'` 문자열
  - 한 줄/블록 주석
  - 숫자 및 이름 바인드
  - 중복 이름 제거와 대소문자 정규화
  - 닫히지 않은 문자열은 `DPY-2041`로 보고
- DML `RETURNING ... INTO`에서 같은 바인드가 입력과 출력에 중복 사용되는 경우를 거부한다.
- `BindValue`에 다음 값을 추가했다.
  - 연결 소유권을 포함한 REF CURSOR 입력 바인드
  - PL/SQL associative array IN/OUT/IN OUT 바인드
- 서로 다른 연결에서 생성한 REF CURSOR 바인드를 거부하고, 닫힌 연결·열리지 않은 커서의 오류를 명확히 처리한다.
- NCHAR 계열 associative array의 charset form과 VARCHAR wire metadata 처리를 보강했다.
- 배열 선언 용량보다 많은 값을 전달하는 경우를 사전에 거부한다.

### 2.3 `execute_many`와 DML RETURNING

- `StatementRequest`에 여러 bind row를 저장하는 `bind_rows`를 추가했다.
- 다음 API와 실행 경로를 구현했다.
  - `OracleThinSession::execute_many`
  - `OracleThinSession::execute_many_out_binds`
- 지원 동작:
  - 여러 행 DML 실행과 누적 row count
  - 긴 문자열 및 대량 bind row
  - DML RETURNING의 실행별 OUT bind 배열 보존
  - 반환 행이 없는 DML RETURNING
  - PL/SQL 반복 실행과 OUT bind 수집
  - 마지막 반복에서만 auto-commit 적용
- 빈 SQL, SELECT 전달, 행마다 다른 바인드 개수 등 잘못된 입력을 실행 전에 거부한다.
- 배치 실행 중 오류가 발생하면 실패한 반복 인덱스를 `offset`에 기록한다.

### 2.4 데이터 타입과 wire 처리

- associative array를 위한 TNS bind array flag, 최대 요소 수, 입력/출력 배열 인코딩과 디코딩을 추가했다.
- DATE/TIMESTAMP 배열, NUMBER/NCHAR 배열, supplementary Unicode 문자열을 왕복 검증했다.
- VECTOR 처리에서 다음 경계를 보강했다.
  - 최대 65,533차원 dense vector 인코딩
  - sparse vector의 index/value 개수 불일치 거부
- Oracle NUMBER의 vendor 허용 범위를 벗어나는 값을 거부한다.
- OSON 디코더의 최대 중첩 깊이를 64로 제한하고 잘못된 payload, 지원하지 않는 버전 및 scalar type을 오류로 처리한다.
- 다중 DML RETURNING 결과와 OUT bind array를 각각의 실행 행에 맞춰 보존한다.

### 2.5 오류 정보와 복구 가능성

- `OracleThinError`에 다음 정보를 추가했다.
  - 숫자 오류 코드
  - `ORA-`, `DPY-`, `DPI-`를 포함한 전체 코드
  - SQL/배치 오류 offset
  - 연결 복구 가능 여부
- 서버가 전달한 오류 위치와 batch offset을 보존한다.
- 세션 종료, 네트워크 단절 및 대표적인 연결 관련 ORA 오류를 recoverable로 분류한다.
- 패킷 읽기 I/O 오류도 연결 복구가 필요한 오류로 표시한다.
- 정상적으로 처리할 수 있는 빈 결과나 내부 상태에서 `expect()`로 panic하지 않도록 오류 반환과 checked 연산으로 변경했다.

### 2.6 커서와 세션 수명

- REF CURSOR를 입력 바인드로 다시 전달하는 경로를 추가했다.
- 중첩 커서, 여러 OUT REF CURSOR, 반복 실행 시 부모/자식 커서 메타데이터와 close 순서를 보강했다.
- 연결 종료 시 미커밋 트랜잭션이 rollback되는 동작을 검증했다.
- compilation warning의 설정·초기화와 DDL/`execute_many` 실행 후 상태를 검증했다.
- call timeout, cancel 후 연결 재사용, 응답 중간 서버 오류를 검증했다.

### 2.7 연결 풀

- idle connection을 LIFO(`pop_back`)로 재사용하도록 정리했다.
- connection hook 또는 상태 확인 전에 pool mutex를 해제하는 기존 동시성 보장을 유지했다.
- 다음 동작을 live test로 검증했다.
  - 최소/최대 pool 크기
  - 동시 acquire
  - LIFO 재사용
  - 반환 시 rollback/reset
  - close 후 동작
  - 기본 max size 4, acquire timeout 5초

### 2.8 Space Query 통합 경로

- Oracle 세션에 알 수 없는 비트랜잭션 residue만 남은 경우에는 `SET TRANSACTION`을 허용하도록 보정했다.
- 다음 상태에서는 기존 차단을 유지한다.
  - dirty transaction
  - session lock
  - transaction mode override
- 이 변경으로 `final.sql`의 `COMMIT` 다음 `SET TRANSACTION`이 OCI와 Thin 양쪽에서 같은 흐름으로 실행된다.

## 3. python-oracledb 테스트 커버리지 관리

- `vendor/python-oracledb/tests`의 source-level test function 2,356개를 전수 인벤토리화했다.
- 각 항목을 다음 중 하나로 기록했다.
  - `covered`: 대응하는 Rust 테스트/함수 식별자를 근거로 연결
  - `not_applicable`: Python 전용 API, thick/OCI 전용 기능, Rust 타입으로 표현 불가능한 호출 등 구체적인 제외 사유 기록
- 커버리지 감사 테스트는 다음 오류를 실패로 처리한다.
  - upstream 테스트 추가/삭제 후 미검토
  - 누락되거나 stale한 매핑
  - 중복 매핑
  - 지원 근거가 실제 Rust 소스에 없는 `covered` 항목
  - 사유가 없는 `not_applicable` 항목
- 최종 결과: 2,356/2,356 검토 완료, 누락·추가·중복 0개.

관련 파일:

- `crates/tns-thin/python_oracledb_coverage.txt`
- `crates/tns-thin/tests/python_oracledb_coverage.rs`
- `crates/tns-thin/tests/live_tns.rs`

## 4. OCI/Thin SQL 비교 강화

### 4.1 전체 fixture 실행

- 기존 `test/test_all.sql` 단일 비교 외에 `test/` 바로 아래의 모든 `.sql`, `.txt` 파일을 정렬해 각각 독립 실행한다.
- 파일마다 OCI와 Thin을 별도 실행한 뒤 오류, 결과 그리드, 컬럼, 행 및 셀 값을 비교한다.
- 현재 대상은 총 42개 파일이다.
- 프로토콜 314, 315, 318, 319 각각에서 같은 전수 비교를 수행한다.

### 4.2 `final.sql` 처리

- `final.sql`은 transaction 제어문 자체를 검증해야 하므로 auto-commit을 끈 상태로 실행한다.
- 스크립트 앞에 `DBMS_RANDOM.SEED` 문장을 삽입하지 않는다.
- 스크립트 실행 전에 별도 SQL을 보내지 않도록 SID와 open cursor 수는 실행 후 조회한다.
- 각 프로토콜에서 다음 결과를 확인했다.
  - 결과 그리드 16개
  - 비교한 셀 285개
  - 문자열까지 정확히 일치한 셀 267개
  - 타입별 의미 동등성으로 일치한 셀 18개
  - 실행 후 open cursor: OCI 2개, Thin 1개

18개 semantic cell은 별도 세션에서 순차 실행할 때 원래 값이 달라질 수 있는 항목이다.

- ROWID: 양쪽 값이 비어 있지 않은지 확인
- 날짜/시간: 양쪽 값이 정상 파싱되고 실행 시각 차이가 허용 범위(600초) 안인지 확인

나머지 결정적 셀은 문자열 표현까지 정확히 같아야 비교가 통과한다.

## 5. 최종 검증 결과

실행 명령:

```bash
./test_tns_thin.sh 314 315 318 319
cargo test
cargo clippy --locked --all-targets -- -D warnings -W clippy::perf -W clippy::complexity
cargo fmt --all
git diff --check
```

결과:

| 검증 | 결과 |
|---|---:|
| TNS Thin core 및 coverage audit | 통과 |
| 프로토콜별 Thin live tests | 213개 통과 |
| 프로토콜별 Space Query live tests | 63개 통과 |
| 프로토콜별 `test_all.sql` OCI/Thin 비교 | 통과 |
| 프로토콜별 SQL/TXT fixture 42개 OCI/Thin 비교 | 통과 |
| `final.sql` 285개 셀 비교 | 4개 프로토콜 모두 통과 |
| `cargo test` | 통과 |
| 지정된 strict Clippy 명령 | 오류·경고 없이 통과 |
| `git diff --check` | 통과 |

일부 테스트는 서버 또는 프로토콜 capability에 따라 명시적으로 건너뛴다. 예를 들어 서버가 PL/SQL BOOLEAN associative array를 거부하거나 프로토콜 314에서 native JSON REF CURSOR를 지원하지 않는 경우다. 이러한 경우는 지원 여부를 먼저 확인하고, 그 외 실패는 테스트 실패로 처리한다.

## 6. 주요 변경 파일

- `crates/tns-thin/src/connect.rs`: Easy Connect와 연결 대상 검증
- `crates/tns-thin/src/exec.rs`: bind 타입, SQL bind parser, DML RETURNING 분석
- `crates/tns-thin/src/lib.rs`: 구조화된 오류 정보와 recoverable 판정
- `crates/tns-thin/src/pool.rs`: LIFO 재사용과 pool 회귀 테스트
- `crates/tns-thin/src/session.rs`: 인증, 세션 메타데이터, execute-many, array/cursor bind 및 wire 처리
- `crates/tns-thin/tests/live_tns.rs`: python-oracledb 대응 live test 확장
- `src/ui/sql_editor/execution.rs`: Oracle transaction option guard 보정
- `src/bin/oracle_compare_test_all.rs`: OCI/Thin 비교와 `final.sql` 처리
- `tests/oracle_compare_test_all_live.rs`: 프로토콜별 전체 fixture 실행
- `test_tns_thin.sh`: core/coverage 및 전체 fixture 일괄 실행
