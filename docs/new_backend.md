# 신규 DB 백엔드 추가 체크리스트

이 문서는 새로운 DB 종류를 추가할 때 개발 누락을 막기 위한 체크리스트다.
세션/트랜잭션/커밋/롤백 동작의 일관성 원칙은 `docs/session.md`,
`docs/transaction.md`를 따른다.

설계 원칙: **컴파일러가 잡을 수 있는 누락은 컴파일러에 맡기고, 이 문서는
컴파일러가 잡지 못하는 부분만 다룬다.** `DatabaseBackendKind`와 `SqlDialect`에
대한 `==`/`!=`/`matches!`/`if let`/`let-else`/와일드카드 arm(중첩 패턴
`Some(_)`, `(_, x)` 포함)은 `tests/db_dispatch_guards.rs`가 금지하므로, 모든
분기 지점은 exhaustive `match`로만 존재한다. 가드가 enum 이름을 텍스트로
인식하므로 별칭/variant import(`use ... DatabaseType as ...`,
`type ... = DatabaseType`, `use DatabaseType::...`)와 dispatch enum impl
내부의 `Self::` variant 패턴(전체 enum 이름을 써야 가드가 인식)도 같은
가드가 금지한다.
물리 세션 enum의 mismatch 처리는 이름 있는 바인딩으로 진단을 남기는 명시적
거부(`Ok(other) => warn(other.db_type())`, divergent let-else)만 허용되고
무명 `_` 폐기는 금지된다. 따라서 enum에 variant를 추가하면 결정이 필요한
지점이 전부 컴파일 에러로 드러난다.

---

## 1. 먼저 결정할 것: backend kind / dialect 공유 여부

새 DB가 기존 family와 프로토콜·SQL 방언·트랜잭션 의미론을 공유하는지 먼저
판단한다.

- **기존 family 재사용** (예: MariaDB가 `DatabaseBackendKind::MySql`을
  재사용): `DatabaseType` variant만 추가하면 되고 비용이 작다.
  family 내부의 차이는 `src/db`의 SQL/프로토콜 의미 차이에만 둔다. 이때도
  `db_type == DatabaseType::MariaDB` 같은 직접 비교가 아니라 wildcard 없는
  exhaustive `match db_type { ... }`를 사용한다. UI 동작은 `DatabaseType`
  직접 비교가 아니라 backend spec/registry를 통해 노출해야 한다
  (`tests/db_dispatch_guards.rs`가 `src/ui` 직접 비교를 금지하고, UI 밖의
  모든 소스(`src/db`뿐 아니라 `sql_text.rs`, `src/bin`, `src/utils` 등)에서
  비exhaustive 구체 타입 분기를 금지한다. `is_same_type_as(DatabaseType::...)`
  같은 메서드형 구체 타입 비교도 직접 비교로 본다).
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
| `object_browser_behavior_for` | `src/ui/object_browser.rs` | 오브젝트 브라우저 동작 |
| `language_catalog_for_db_type` | `src/ui/intellisense.rs` | 인텔리센스 키워드/함수 카탈로그 |
| `function_catalog_for_db_type` / `mysql_compatible_highlight_mode` | `src/ui/syntax_highlight.rs` | 구문 강조 카탈로그/모드 |

UI의 `*_backend_for`/`*_behavior_for`/`*_loader_for` registry 함수는 예외적으로
concrete `DatabaseType` exhaustive match를 사용한다. 새 DB가 기존
`DatabaseBackendKind`를 공유하더라도 이 registry들이 컴파일 에러를 내며, 그 DB가
정말 기존 UI backend를 공유해도 되는지 직접 확인하게 하기 위해서다. 일반 UI
코드는 여전히 `DatabaseType` 직접 분기 대신 backend spec/registry를 사용한다.

그 외 exhaustive match 또는 guard로 누락이 드러나는 곳: `DatabaseType::ALL`
(`tests/db_dispatch_guards.rs`가 enum variant와 배열을 비교),
`DbConnection`/`DbConnectionPool`/`DbPoolSession`/`DbSessionLease` enum의 물리
세션 생명주기 분기(`tests/db_dispatch_guards.rs`가 `matches!`/`if let`/wildcard
arm을 금지),
`sql_classification.rs`의 family bool 바인딩들, dialect별 키워드/함수
카탈로그(`sql_text.rs`, `syntax_highlight.rs`, `intellisense.rs` — UI 쪽
카탈로그 함수는 위 registry 표에 포함되어 guard가 concrete dispatch를 강제),
`cache_key()` 유일성과 `from_cache_key` 라운드트립
(`tests/db_dispatch_guards.rs`의 런타임 테스트가 중복 키를 잡는다),
에러 메시지 마커 카탈로그(`src/db/session_policy.rs`의
`query_cancel_markers_for_db_type` / `connection_loss_markers_for_db_type` /
`error_line_patterns_for_db_type` — 범용 UI(result table, query history)는
db_type을 모르는 채 이 카탈로그의 합집합으로 취소/연결 끊김/에러 라인을
분류하므로, 새 DB의 드라이버 에러 텍스트 마커를 여기에 등록해야 한다.
`tests/db_dispatch_guards.rs`의
`ui_source_does_not_hardcode_driver_error_markers`가 generic UI의 `ORA-`/`DPI-`
류 드라이버 마커 리터럴 하드코딩을 금지한다. 백엔드 전용 worker가 있는
`src/ui/sql_editor/execution.rs`만 예외).

## 3. 컴파일러가 잡지 못하는 것 (수동 확인 필수)

### 3.1 `DbBackend` 구현

advanced 설정과 스코프/트랜잭션/세션 동작 메서드(`default_advanced_settings`,
`validate_advanced_settings`, `apply_current_scope_to_session`, `current_scope_name`,
`switch_scope`, `apply_scope_to_lease`, `has_connection_scope`,
`can_apply_empty_scope_to_retained_session`,
`retained_session_blocks_transaction_mode_change`,
`can_replace_retained_transaction_mode`, `metadata_scope_noun`, `switch_scope_noun`,
`after_connect`, `apply_auto_commit`,
`supports_mysql_delimiter_commands`,
`apply_transaction_mode_to_live_connection`,
`read_current_default_transaction_isolation`,
`transaction_mode_requires_first_statement`, `transaction_mode_statements`,
`is_recoverable_timeout_message` 등)는 default 본문이 없으므로 구현이
강제되지만, **"동작이 없음"도 명시적 결정이어야 한다.** no-op으로 구현할
때는 왜 no-op이 올바른지 주석으로 남긴다(예: Oracle은 auto-commit을
세션 플래그가 아니라 실행 시점에 적용).
`DbBackend`의 default method body는 label/message formatting처럼 다른 required
method에서 계산되는 파생 helper만 허용한다. 새 정책 default를 추가하면
`tests/db_dispatch_guards.rs`가 실패해야 한다.

### 3.2 SQL 분류 (`src/db/sql_classification.rs`)

`DbExecutionBackend::profile_statement`와 `SqlKind` 분류는 결과 라우팅,
트랜잭션 상태 추적, 세션 재사용 정책의 입력이다. `query_timeout_for_statement`는
statement 자체가 timeout/session 변수를 만지는 경우와 UI timeout 적용이 충돌하지
않도록 정한다. 두 메서드는 default 구현이 없으므로 새 backend가 직접 정책을
확인해야 한다. 새 family는
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

커밋/롤백/폐기 액션은 retained physical session 정책을 직접 재구현하지 말고
`ensure_retained_session_resolution_action_allowed`,
`ensure_retained_session_transaction_action_allowed`,
`retained_session_transaction_resolution_should_discard_after_success`를 사용한다. 이
함수들이 transaction-only action과 cleanup/discard-only session state의 차이를
한 곳에서 유지한다.
`StatementSessionPostProcessor`의 default method body도 공통 보존/결정 계산
helper만 허용된다. backend별 효과 산출(`effects_for_sql`)은 반드시 직접
구현한다.

### 3.5 설정/UI

- `ConnectionAdvancedSettings`에 백엔드 전용 필드 추가 (serde 저장 포맷이므로
  기존 필드는 건드리지 않는다. `#[serde(default)]` 필수)
- `DbConnectionFormSpec` / `DbAdvancedSettingsFormSpec`에 표시 플래그 추가
  (예: driver row, TNS alias, DB별 advanced row). UI 코드에서
  `DatabaseType::Oracle` 같은 직접 비교로 행 표시를 결정하지 않는다.
- `cache_key()`는 기존 값과 충돌하지 않는 새 값 할당 (Oracle=0, MySQL=1,
  MariaDB=2). 중복/비라운드트립은 `tests/db_dispatch_guards.rs`의
  `database_type_cache_keys_are_unique_and_round_trip`이 잡는다.
- object browser 동작(`ObjectBrowserDbBehavior`)은 package routine,
  compilation status, DDL/action 메뉴 지원 여부까지 backend별로 직접 구현한다.
  지원하지 않는 기능도 default 상속이 아니라 명시적인 unsupported 메시지로 둔다.
- UI backend registry 함수에 새 variant를 추가한다. 기존 family backend를
  공유하는 경우에도 `DatabaseType::NewDb => &MYSQL_...`처럼 명시적으로 매핑한다.
- formatter의 DB 추론 fallback은 UI에서 직접 고르지 말고
  `sql_text::format_preferred_db_type_for_sql`에 둔다. 새 dialect를 추가하면 이
  helper의 대표 DB 매핑과 "preferred DB가 없을 때 어떤 formatter를 쓰는가"를
  함께 검토한다.

### 3.6 그리드 편집

그리드 편집(result table edit mode)은 실행 워커가 결과에 행 식별자 컬럼을
주입했는지로 활성화가 결정된다(Oracle: `maybe_inject_rowid_for_editing`이
`SQ_INTERNAL_ROWID` alias로 ROWID를 주입하고 워커가 표시 시 `ROWID`로
정규화). 컴파일 에러로 드러나지 않으므로 새 backend는 다음을 직접 결정한다:

- 그리드 편집을 지원하는가. 지원하면 워커의 `begin_execution`에서 행 식별자
  주입과 alias 정규화를 구현한다. 지원하지 않으면 주입하지 않는 것으로
  충분하다(식별자 컬럼이 없으면 편집 액션이 노출되지 않는다) — 단, 결정을
  워커 구현 주석으로 남긴다.
- 저장(save) 결과 라우팅: 편집 탭에서 실행된 non-select 저장 DML 결과가
  편집 탭의 result table로 돌아와야 저장 pending이 해제된다
  (`src/bin/verify_grid_save_live.rs`로 검증).
- 저장 실패/중단 분류는 `db::session_policy`의 메시지 분류기
  (`message_indicates_query_cancel` / `message_indicates_execution_abort` /
  `message_indicates_connection_loss`)를 거친다. 새 DB의 마커는 2장의
  에러 메시지 마커 카탈로그에 등록한다.

### 3.7 테스트

- `tests/db_dispatch_guards.rs`가 통과하는지 확인 (boolean 분기 금지)
- `src/db/query/query_tests.rs`에 새 db_type 분류/정책 테스트 추가
- 라이브 테스트 가능하면 docker 기반 라이브 테스트 추가
  (`tests/oracle_compare_test_all_live.rs` 참고)
