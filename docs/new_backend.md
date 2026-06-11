# 신규 DB 백엔드 추가 체크리스트

이 문서는 새로운 DB 종류를 추가할 때 개발 누락을 막기 위한 체크리스트다.
세션/트랜잭션/커밋/롤백 동작의 일관성 원칙은 `docs/session.md`,
`docs/transaction.md`를 따른다.

설계 원칙: **컴파일러가 잡을 수 있는 누락은 컴파일러에 맡기고, 이 문서는
컴파일러가 잡지 못하는 부분만 다룬다.** `DatabaseBackendKind`와 `SqlDialect`에
대한 `==`/`!=`/`matches!`/`if let`/와일드카드 arm은
`tests/db_dispatch_guards.rs`가 금지하므로, 모든 분기 지점은 exhaustive
`match`로만 존재한다. 따라서 enum에 variant를 추가하면 결정이 필요한 지점이
전부 컴파일 에러로 드러난다.

---

## 1. 먼저 결정할 것: backend kind / dialect 공유 여부

새 DB가 기존 family와 프로토콜·SQL 방언·트랜잭션 의미론을 공유하는지 먼저
판단한다.

- **기존 family 재사용** (예: MariaDB가 `DatabaseBackendKind::MySql`을
  재사용): `DatabaseType` variant만 추가하면 되고 비용이 작다.
  family 내부의 차이는 `db_type == DatabaseType::MariaDB` 같은 구체 타입
  비교로 처리한다(새 variant에 false로 떨어지는 것이 베이스 동작이므로 안전).
- **새 family** (예: PostgreSQL): `DatabaseBackendKind`와 `SqlDialect`에
  variant를 추가한다. 이 경우 아래 모든 단계가 필요하다.

## 2. 컴파일 에러가 안내하는 지점 (참고용 목록)

variant 추가 후 컴파일 에러가 발생하는 디스패치 레지스트리. 각각에 새 백엔드
구현을 등록한다.

| 디스패치 함수 | 위치 | 역할 |
|---|---|---|
| `backend_for` | `src/db/connection.rs` | `DbBackend` — 연결/풀/트랜잭션 옵션/scope |
| `db_execution_backend_for` | `src/db/query/execution_backend.rs` | statement 분류/타임아웃 프로파일 |
| `statement_session_post_processor_for` | `src/db/transaction.rs` | statement 후 세션 상태 힌트 |
| `execution_worker_backend_for` | `src/ui/sql_editor/execution.rs` | 실행 워커 진입점 |
| `transaction_action_backend_for` | `src/ui/sql_editor/mod.rs` | commit/rollback/discard 액션 |
| `explain_plan_backend_for` | `src/ui/sql_editor/mod.rs` | 실행 계획 |
| `schema_metadata_loader_for` | `src/ui/main_window.rs` | 오브젝트 브라우저 메타데이터 |
| `quick_describe_backend_for` | `src/ui/sql_editor/intellisense/popup.rs` | quick describe |
| `signature_backend_for` | `src/ui/sql_editor/intellisense/popup.rs` | 프로시저 시그니처 |
| `column_load_backend_for` | `src/ui/sql_editor/intellisense/helpers.rs` | 컬럼 로드 |

그 외 exhaustive match로 컴파일 에러가 나는 곳: `DatabaseType::ALL`,
`DbConnection`/`DbConnectionPool`/`DbPoolSession`/`DbSessionLease` enum
(`src/db/connection.rs` 내부에 격리됨), `sql_classification.rs`의 family
bool 바인딩들, dialect별 키워드/함수 카탈로그(`sql_text.rs`,
`syntax_highlight.rs`, `intellisense.rs`).

## 3. 컴파일러가 잡지 못하는 것 (수동 확인 필수)

### 3.1 `DbBackend` 구현

트랜잭션/세션 동작 메서드(`after_connect`, `apply_auto_commit`,
`apply_transaction_mode_to_live_connection`,
`read_current_default_transaction_isolation`,
`transaction_mode_requires_first_statement`, `transaction_mode_statements`,
`is_recoverable_timeout_message` 등)는 default 본문이 없으므로 구현이
강제되지만, **"동작이 없음"도 명시적 결정이어야 한다.** no-op으로 구현할
때는 왜 no-op이 올바른지 주석으로 남긴다(예: Oracle은 auto-commit을
세션 플래그가 아니라 실행 시점에 적용).

### 3.2 SQL 분류 (`src/db/sql_classification.rs`)

`SqlKind` 분류는 트랜잭션 상태 추적·세션 재사용 정책의 입력이다. 새 family는
키워드별로 직접 결정해야 한다:

- 어떤 문장이 implicit commit을 유발하는가 (DDL 등)
- 어떤 문장이 세션 잔여물(temp table, prepared statement, user variable,
  lock)을 남기는가
- 어떤 문장이 transaction control / session control인가

보수적 기본값: 불확실하면 `SqlKind::Unknown`(인터럽트 시 세션 폐기)으로
분류한다. `docs/session.md`의 "위험하거나 불확실한 물리 세션은 폐기" 원칙을
따른다.

### 3.3 실행 워커 (`src/ui/sql_editor/execution.rs`)

`ExecutionWorkerBackend::begin_execution`을 구현한다. 백엔드 전용 상태
슬롯(cancel 컨텍스트 등)이 필요하면 trait 시그니처를 바꾸지 말고
`ExecutionWorkerContext`에 필드를 추가한다. 구현 시 `docs/session.md`의
cancel/timeout/lazy fetch/세션 유지 정책을 항목별로 대조한다.

### 3.4 executor

`src/db/query/executor.rs`(Oracle), `mysql_executor.rs`(MySQL family)에
상응하는 executor를 새로 작성한다. 공유 trait이 없으므로 다음 계약을 수동으로
지켜야 한다:

- 커서/리소스 정리 책임 (OCI와 thin의 커서 계약 차이 참고)
- query timeout 적용과 recoverable timeout 분류
- lazy fetch 배치 계약
- auto-commit 의미론 (statement 단위 vs 세션 플래그)

결과 메시지와 트랜잭션 피드백 조립은 `src/db/query/types.rs`의
`result_messages` 공유 계층을 거친다: `dml_rows_affected`, `with_out_binds`,
`apply_transaction_feedback`(정책은 `transaction_feedback_flag`).
어떤 statement가 "| Auto-commit applied" / "| Commit required"를 보고하는지는
`transaction_feedback_flag`의 exhaustive match 한 곳에만 존재하므로, 새
family를 추가하면 이 함수에서 컴파일 에러로 피드백 정책 결정이 강제된다.
실행기에서 이 텍스트들을 인라인으로 조립하지 않는다.

### 3.5 설정/UI

- `ConnectionAdvancedSettings`에 백엔드 전용 필드 추가 (serde 저장 포맷이므로
  기존 필드는 건드리지 않는다. `#[serde(default)]` 필수)
- `DbConnectionFormSpec` / `DbAdvancedSettingsFormSpec`에 표시 플래그 추가
- `cache_key()`는 기존 값과 충돌하지 않는 새 값 할당 (Oracle=0, MySQL=1,
  MariaDB=2)

### 3.6 테스트

- `tests/db_dispatch_guards.rs`가 통과하는지 확인 (boolean 분기 금지)
- `src/db/query/query_tests.rs`에 새 db_type 분류/정책 테스트 추가
- 라이브 테스트 가능하면 docker 기반 라이브 테스트 추가
  (`tests/oracle_compare_test_all_live.rs` 참고)
