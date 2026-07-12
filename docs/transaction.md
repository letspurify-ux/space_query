# 트랜잭션 및 retained session 관리

> 구현 대조: 2026-07-12 (`src/db/transaction.rs`, `src/db/session_policy.rs`)

## 목적

SPACE Query는 트랜잭션 상태를 에디터 문자열이나 UI 표시가 아니라 실제 물리 DB
세션에 연결한다. commit/rollback, 세션 재사용, scope 변경, 탭 닫기 정책은 모두
해당 물리 세션의 `RetainedSessionState`를 기준으로 결정한다.

핵심 원칙은 다음과 같다.

```text
확실하지 않은 세션을 Clean으로 낮추지 않는다.
사용자 작업이나 session-bound state가 남아 있으면 같은 물리 세션을 보존한다.
보존할 필요가 없는 불확실한 세션은 논리 연결을 유지한 채 폐기한다.
```

## 상태 모델

트랜잭션 상태는 `TransactionSessionState`로 표현한다.

| 상태 | 의미 |
| --- | --- |
| `Clean` | 알려진 미커밋 작업이 없음 |
| `MaybeDirty` | 알려진 동일 트랜잭션을 계속 사용할 수 있으며 물리 세션 보존이 필요함 |
| `BlockedDirty` | 상태가 불확실해 일반 실행 전에 해결이 필요함 |
| `DecisionRequired` | commit/rollback/discard 사용자 결정이 필요함 |
| `InvalidSession` | 재사용할 수 없는 물리 세션 |

`RetainedSessionState`는 이 트랜잭션 상태 하나만 저장하지 않는다.

- `TransactionSessionState`
- `SessionResidueState`
  - temporary table
  - prepared statement
  - user variable
  - 다음/현재 transaction mode override
  - 추적되지 않은 session state
- `SessionLockState`
  - table lock
  - flush table lock
  - backup lock
  - named lock

따라서 `MaybeDirty`라는 요약 label만 보고 commit/rollback 가능 여부를 추론하면
안 된다. 실제 UI 동작은 `RetainedSessionCapabilities`를 사용한다.

## capability 기반 동작

`RetainedSessionState::capabilities()`는 다음 결정을 제공한다.

- commit/rollback 가능 여부
- 물리 세션 discard 가능 여부
- transaction 해결 후에도 residue/lock 때문에 세션을 버려야 하는지
- auto-commit/isolation/access mode 변경 가능 여부
- 다음 일반 실행을 막아야 하는지

예를 들어 session residue만 남아 있는 상태는 같은 물리 세션에서 명시적 cleanup을
실행할 수 있으므로 항상 다음 실행을 막지는 않는다. 반면 session lock, 불확실한
transaction, 아직 소비되지 않은 one-shot transaction mode override는 실행이나
옵션 변경을 막을 수 있다.

## 탭별 물리 세션

- 실행에 사용된 lease는 해당 query tab이 retained session으로 보존할 수 있다.
- commit/rollback 대상은 요청 시점의 선택 탭과 물리 세션으로 고정한다.
- `connection_generation`이 달라진 stale lease는 재사용하지 않는다.
- 동일 connection generation 안에서도 pool context epoch, DB type, scope가 맞아야
  재사용할 수 있다.
- scope 변경이나 연결 전환 전에 보존이 필요한 세션이 있으면 중앙 preflight가
  사용자 해결을 요구한다.

## 중앙 preflight

`RetainedSessionPreflightAction`은 다음 동작을 구분한다.

- `Execute`
- `TransactionOptionChange`
- `ScopeChange`
- `ConnectionTransition`
- `PoolResize`
- `Close`
- `ReleaseClean`
- `Discard`

결과는 `Allow` 또는 `RequireResolution`이다. SQL text가 있는 실행 경로는
`retained_session_state_execute_preflight_decision_for_sql()`을 사용한다. 이 함수는
blocked session이라도 현재 문장이 lock/session residue를 정리하거나 MySQL/MariaDB
one-shot transaction mode를 소비하는 문장임을 증명하면 실행을 허용한다.

## Auto-commit과 transaction mode

- DB 적용이 성공한 뒤에만 전역 설정과 UI 상태를 변경한다.
- auto-commit 또는 transaction mode 변경은 모든 retained session의 capability를
  먼저 검사한다.
- 변경할 수 없는 dirty/locked/unknown session이 있으면 commit, rollback, discard
  또는 명시적 cleanup을 요구한다.
- pool context에는 auto-commit과 transaction mode가 포함되며 설정 변경 시 epoch를
  무효화해 오래된 pooled context 재사용을 막는다.
- Oracle의 read-only/serializable 설정은 first-statement 제약을 따른다.
- MySQL/MariaDB의 `SET TRANSACTION` one-shot override는 같은 물리 세션에서 다음
  transaction-starting statement가 소비할 때까지 보존한다.

## statement 후 상태 전이

SQL 분류와 상태 전이는 `src/db/sql_classification.rs`와
`StatementSessionPostProcessor`를 통해 DB별로 처리한다.

대표 규칙:

- 성공한 `COMMIT`/`ROLLBACK`은 transaction state를 clean으로 만든다.
- `BEGIN`/`START TRANSACTION`, DML, `SAVEPOINT`, `ROLLBACK TO SAVEPOINT`는 실제
  transaction 상태와 auto-commit을 반영해 보존 여부를 결정한다.
- DDL의 implicit commit 의미는 DB별 profile에서 처리한다.
- temporary table, prepared statement, user variable, table/named lock 문장은
  transaction과 별도로 residue/lock state를 남긴다.
- PL/SQL/CALL처럼 session state를 바꿀 수 있는 문장은 성공했더라도 보수적인
  residue를 남길 수 있다. 성공한 문장이라는 이유만으로 다음 실행을 무조건
  block하지는 않는다.

수동 transaction/session control을 일반 DML처럼 처리하거나 UI에서 문자열만 보고
상태를 직접 바꾸면 안 된다.

## cancel/timeout 후 처리

interrupt 후 결정은 `decide_session_after_interrupt()`와 실제 worker cleanup 결과를
사용한다.

- 안전한 SELECT cancel/recoverable timeout은 cursor close, worker 종료, timeout
  복구, health check가 모두 성공하면 같은 세션을 재사용할 수 있다.
- DML/PLSQL/script interrupt에서 미커밋 가능성이 있으면 commit/rollback/discard를
  요구한다.
- transaction보다 session residue/lock 해결이 필요한 경우 commit/rollback을
  잘못 제시하지 않고 physical-session resolution/discard를 요구한다.
- connection-fatal 오류나 cleanup 실패는 물리 세션을 폐기한다.
- health check 성공은 연결 생존만 뜻하며 `Clean` 증거가 아니다.

세부 cancel/lazy fetch 정책은 `docs/session.md`를 따른다.

## 사용자 해결 동작

`RetainedSessionResolutionAction`은 다음 3개다.

- `Commit`
- `Rollback`
- `DiscardPhysical`

commit/rollback은 capability가 허용하는 경우에만 실행한다. transaction 해결에
성공했더라도 session residue, lock, transaction mode override가 남아 있으면
`discard_after_transaction_resolution` 정책에 따라 물리 세션을 폐기한다.

탭 닫기, 앱 종료, 연결 해제, 연결 전환, pool resize에서도 같은 중앙 정책을
사용한다. 미커밋 작업이 있는 세션을 사용자 결정 없이 조용히 commit/rollback하지
않는다.

## 새 문장/백엔드 추가 시 체크리스트

- [ ] SQL kind와 implicit commit 의미를 DB별로 분류했다.
- [ ] transaction, residue, lock 효과를 `StatementSessionEffects`에 반영했다.
- [ ] cancel 가능한 문장인지와 interrupt 후 보존 정책을 정의했다.
- [ ] auto-commit/transaction mode 적용 실패를 `Result`로 전파한다.
- [ ] commit/rollback 대상이 요청 시점의 retained session으로 고정된다.
- [ ] health check와 transaction clean 판정을 분리한다.
- [ ] scope/connection/pool 변경 preflight를 우회하지 않는다.
- [ ] cleanup-only 상태에서 commit/rollback을 잘못 노출하지 않는다.
- [ ] `tests/db_dispatch_guards.rs`와 transaction/session policy 테스트를 추가한다.

## 검증

```sh
cargo test transaction --lib
cargo test session_policy --lib
cargo test --test concurrency_multithread_guards
cargo test --test db_dispatch_guards
```
