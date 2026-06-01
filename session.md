# DB 세션 관리 원칙

이 문서는 쿼리 탭과 DB 세션의 소유권, `cancel`, `timeout`, `lazy fetch`, transaction, 세션 재사용 정책을 정의한다.

이 문서의 목적은 cancel 로직을 개선할 때 다음 요구사항을 일관되게 구현하도록 하는 것이다.

- Cancel 후 UI의 논리 연결 상태는 가능한 한 유지한다.
- 실제 물리 DB 세션은 안전하다고 판단되는 경우에만 재사용한다.
- 위험하거나 불확실한 물리 세션은 폐기하고, 다음 실행 시 새 물리 세션을 자동 획득한다.
- lazy fetch `waiting` / `fetching` 상태와 recoverable timeout도 살릴 수 있는 것은 살린다.
- DML, PL/SQL, script, transaction 오염 가능성이 있는 세션은 보수적으로 처리한다.

---

## 1. 핵심 원칙

Cancel 또는 timeout 후 처리의 최우선 원칙은 다음과 같다.

```text
논리 세션은 유지한다.
물리 세션은 안전하게 정리되고 검증된 경우에만 재사용한다.
```

따라서 Cancel 후 바로 `Disconnected`로 표시하지 않는다.

물리 세션을 폐기하더라도 UI는 기본적으로 다음 상태를 유지한다.

```text
Cancelled | Connected
```

새 물리 세션 획득에 실패한 경우에만 UI를 `Disconnected`로 전환한다.

---

## 2. 용어

### 2.1 논리 세션

사용자가 UI에서 인식하는 연결 상태다.

예:

- 상태바의 `Connected`
- 쿼리 탭의 연결 대상
- 현재 schema / database 표시
- 탭이 연결된 DB profile 정보

논리 세션은 Cancel 때문에 쉽게 끊으면 안 된다.

### 2.2 물리 세션

실제 DB connection 또는 driver connection handle이다.

예:

- Oracle `Connection`
- MySQL / MariaDB `PooledConn`
- pool에서 lease한 DB session
- lazy fetch가 cursor와 함께 붙잡고 있는 DB session

물리 세션은 cancel, timeout, connection error, cursor 정리 실패가 있으면 폐기될 수 있다.

### 2.3 실행 세션

일반 SQL 실행 worker가 사용 중인 물리 세션이다.

### 2.4 lazy fetch 세션

결과 그리드에서 추가 row fetch를 위해 열린 cursor와 함께 유지되는 물리 세션이다.

### 2.5 dirty session

다음 중 하나라도 해당하면 dirty session으로 본다.

- 미커밋 DML 가능성이 있음
- PL/SQL 또는 procedure가 중간에 cancel / timeout됨
- script 실행 중 일부 statement만 실행됐을 수 있음
- 열린 cursor가 정상 close됐는지 확인 불가
- fetch worker가 종료됐는지 확인 불가
- connection error가 발생함
- timeout cleanup 성공 여부가 불확실함
- transaction 상태를 신뢰할 수 없음

dirty session은 같은 물리 세션으로 즉시 재사용하면 안 된다.

### 2.6 current schema / database

Current schema/database는 connection-level logical state로 본다.

- Oracle `ALTER SESSION SET CURRENT_SCHEMA = ...` 성공 시 tracked current schema를 갱신한다.
- MySQL/MariaDB `USE ...` 또는 DB scope switch 성공 시 tracked current database를 갱신한다.
- 변경 성공 후 같은 `connection_generation`의 새 물리 세션과 모든 retained session에 tracked current schema/database를 적용한다.
- 실행 중인 worker 또는 lazy fetch가 붙잡고 있는 물리 세션은 중간에 강제로 변경하지 않고, 다음 사용 전에 tracked current schema/database를 적용한다.
- dirty 또는 decision-required retained session에 scope를 적용해도 transaction 상태나 decision-required 상태를 해제하면 안 된다.
- 변경 성공 시 사용자가 알 수 있도록 current schema/database 변경 메시지를 출력한다.

---

## 3. 세션 소유권

- 세션은 탭별로 유지한다.
- 탭에서 첫 쿼리를 실행할 때 세션을 생성한다.
- 세션이 생성된 뒤에는 해당 쿼리 탭이 세션의 소유자다.
- `commit`, `rollback`은 명령을 요청한 순간 선택된 탭의 세션에 적용한다.
- 사용자가 `commit` 또는 `rollback`을 누른 뒤 선택 탭이 바뀌어도, 이미 요청된 명령의 대상 세션은 바뀌면 안 된다.
- `cancel`은 명령 요청 시점에 실행 중이거나 lazy fetch 중인 세션을 대상으로 한다.
- cancel 대상은 요청 시점에 snapshot으로 고정한다.
- snapshot 이후 새로 시작된 실행 또는 fetch는 기존 cancel 요청의 대상이 아니다.

---

## 4. Cancel 대상 snapshot

Cancel 버튼을 누르면 즉시 cancel 대상 snapshot을 만든다.

snapshot에는 가능한 한 다음 정보를 포함한다.

```rust
struct CancelTargetSnapshot {
    tab_id: TabId,
    editor_id: EditorId,
    operation_id: u64,
    connection_generation: u64,
    db_type: DatabaseType,
    sql_kind: SqlKind,
    execution_state: ExecutionState,
    lazy_state: LazyFetchState,
    autocommit: bool,
}
```

`operation_id` 또는 `connection_generation`이 현재 상태와 맞지 않으면 완료 이벤트를 무시한다.

이 규칙은 다음 문제를 막기 위한 것이다.

- 탭 A cancel이 탭 B 실행을 끊는 문제
- 이전 실행의 완료 이벤트가 새 실행 상태를 덮어쓰는 문제
- 이미 교체된 물리 세션에 대해 늦게 도착한 cancel 결과가 적용되는 문제
- result tab이 닫힌 뒤 도착한 lazy fetch 완료 이벤트가 잘못 적용되는 문제

---

## 5. 실행 상태

Cancel 판단에 필요한 실행 상태는 다음처럼 분류한다.

```rust
enum ExecutionState {
    Idle,
    RunningStatement,
    RunningScript,
    LazyFetchOnly,
    CancelRequested,
    ClosingCursor,
    Finished,
    Unknown,
}
```

`Unknown`은 안전하지 않은 상태로 본다.

```text
ExecutionState::Unknown → 물리 세션 교체
```

---

## 6. SQL 종류 분류

SQL은 최소한 다음처럼 분류한다.

```rust
enum SqlKind {
    SelectLike,
    Dml,
    Ddl,
    PlsqlOrProcedure,
    Script,
    TransactionControl,
    Unknown,
}
```

### 6.1 SelectLike

다음은 SELECT 계열로 본다.

- `SELECT`
- read-only `WITH ... SELECT`
- DB별 read-only explain query
- lazy fetch가 붙은 SELECT 결과

단, lazy fetch가 있으면 일반 SELECT보다 더 엄격하게 검증한다.

### 6.2 DML

다음은 DML로 본다.

- `INSERT`
- `UPDATE`
- `DELETE`
- `MERGE`
- bulk DML

### 6.3 DDL

다음은 DDL로 본다.

- `CREATE`
- `ALTER`
- `DROP`
- `TRUNCATE`
- `RENAME`

### 6.4 PL/SQL 또는 procedure

다음은 실행 범위가 불명확할 수 있으므로 보수적으로 본다.

- Oracle anonymous block
- `BEGIN ... END`
- `CALL`
- stored procedure 실행
- side effect를 가질 수 있는 function 실행

### 6.5 Script

여러 statement를 순차 실행하는 경우 script로 본다.

script는 일부 statement만 실행됐을 수 있으므로 cancel / timeout 후 같은 물리 세션 재사용을 기본 금지한다.

### 6.6 Unknown

분류할 수 없는 SQL은 안전하지 않은 것으로 본다.

```text
SqlKind::Unknown → 물리 세션 교체
```

---

## 7. Lazy Fetch 상태

lazy fetch는 SELECT 결과를 가져오는 과정이지만, 세션 재사용 판단에서는 일반 SELECT와 다르게 본다.

```rust
enum LazyFetchState {
    None,
    Waiting,
    Fetching,
    CloseRequested,
    CancelRequested,
    Closed,
    Unknown,
}
```

### 7.1 None

lazy fetch가 없다.

일반 SELECT cancel이면 worker 종료 후 health check 성공 시 같은 물리 세션을 재사용할 수 있다.

### 7.2 Waiting

cursor는 열려 있지만 현재 fetch 작업은 돌고 있지 않은 상태다.

이 상태는 세션을 살릴 수 있다.

처리 흐름:

```text
lazy fetch waiting
→ lazy fetch worker에 GracefulClose 요청
→ worker가 cursor close
→ LazyFetchClosed 수신
→ health check
→ 성공하면 같은 물리 세션 재사용
→ 실패하면 물리 세션 교체
```

중요:

- UI thread가 직접 cursor를 닫으면 안 된다.
- cursor를 소유한 lazy fetch worker가 닫아야 한다.
- worker 완료 이벤트를 받기 전에는 물리 세션을 재사용하면 안 된다.

### 7.3 Fetching

worker가 row fetch를 수행 중인 상태다.

이 상태도 가능한 경우 세션을 살린다.

처리 흐름:

```text
lazy fetch fetching
→ lazy fetch worker에 CancelFetch 요청
→ fetch 중단
→ cursor close
→ LazyFetchClosed 수신
→ health check
→ 성공하면 같은 물리 세션 재사용
→ 실패하면 물리 세션 교체
```

fetching 상태에서 다음 중 하나라도 실패하면 물리 세션을 폐기한다.

- worker 종료 확인 실패
- cursor close 확인 실패
- timeout이 non-recoverable로 분류됨
- health check 실패
- connection error 발생
- operation_id 또는 connection_generation 불일치

### 7.4 CloseRequested / CancelRequested

정리 요청을 보냈지만 완료가 확인되지 않은 상태다.

이 상태에서는 아직 물리 세션 재사용 여부를 결정하지 않는다.

다음 이벤트를 기다린다.

```text
LazyFetchClosed
ExecutionFinished
Timeout / Error
```

완료 이벤트 없이 다음 실행을 허용하면 안 된다.

### 7.5 Closed

cursor close와 worker 종료가 확인된 상태다.

다른 재사용 조건을 만족하고 health check가 성공하면 물리 세션을 재사용할 수 있다.

### 7.6 Unknown

상태를 판단할 수 없으면 안전하지 않다.

```text
LazyFetchState::Unknown → 물리 세션 교체
```

---

## 8. Lazy Fetch command 정책

기존 cancel과 별도로 lazy fetch 정리 명령을 분리한다.

권장 command:

```rust
enum LazyFetchCommand {
    FetchMore(usize),
    FetchAll,
    GracefulClose,
    CancelFetch,
    ForceCancel,
}
```

### 8.1 GracefulClose

waiting 상태에서 사용한다.

목표:

- 새 row fetch를 시작하지 않음
- 열린 cursor만 안전하게 닫음
- 물리 세션 재사용 가능성을 최대한 보존함

### 8.2 CancelFetch

fetching 상태에서 사용한다.

목표:

- 현재 fetch를 중단함
- cursor close를 시도함
- 완료되면 세션 재사용 가능성을 판단함

### 8.3 ForceCancel

worker가 응답하지 않거나 timeout된 경우 사용한다.

목표:

- 세션을 살리는 것이 아니라 안전하게 포기함
- 물리 세션은 폐기 대상으로 표시함

---

## 9. Interrupt 종류

Cancel, timeout, connection error는 서로 다르게 분류한다.

```rust
enum InterruptKind {
    None,
    Cancelled,
    RecoverableTimeout,
    NonRecoverableTimeout,
    ConnectionError,
    UnsafeOrUnknown,
}
```

### 9.1 Cancelled

사용자 Cancel 또는 DB cancel 에러다.

예:

```text
Oracle:
- ORA-01013
- Query cancelled

MySQL / MariaDB:
- error 1317
- query execution was interrupted
- query was killed
```

### 9.2 RecoverableTimeout

timeout이지만 같은 물리 세션을 살려볼 수 있는 경우다.

예:

```text
Oracle:
- DPI-1067

MySQL:
- error 3024
- ER_QUERY_TIMEOUT
- maximum statement execution time exceeded
- max_execution_time

MariaDB:
- max_statement_time
- maximum statement execution time exceeded
```

단, recoverable timeout이라도 다음 조건을 만족해야만 재사용 가능하다.

- SELECT 계열이다.
- DML / DDL / PL/SQL / script가 아니다.
- worker 종료가 확인됐다.
- cursor close가 확인됐다.
- connection error가 아니다.
- timeout 설정 복구가 성공했다.
- health check가 성공했다.

### 9.3 NonRecoverableTimeout

timeout 후 connection 또는 driver 상태를 신뢰할 수 없는 경우다.

예:

```text
Oracle:
- ORA-3114
- ORA-03113
- ORA-03114

MySQL / MariaDB:
- server has gone away
- lost connection
- commands out of sync
- connection reset
- broken pipe
- socket timeout
- network timeout
- read timeout
- write timeout
```

Non-recoverable timeout은 같은 물리 세션을 재사용하지 않는다.

### 9.4 ConnectionError

cancel도 아니고 recoverable timeout도 아닌 연결 오류다.

예:

```text
Oracle:
- ORA-03113
- ORA-03114
- ORA-03135
- ORA-12170
- ORA-125xx
- not connected
- closed connection
- connection reset
- broken pipe
- TNS:

MySQL / MariaDB:
- error 2006
- error 2013
- server has gone away
- lost connection
- commands out of sync
- packet out of order
- unexpected eof
- connection reset
- broken pipe
```

### 9.5 UnsafeOrUnknown

분류할 수 없는 interrupt는 안전하지 않은 것으로 본다.

```text
InterruptKind::UnsafeOrUnknown → 물리 세션 교체
```

---

## 10. Error classification 규칙

error classification은 다음 순서를 지킨다.

```text
1. 명시적인 fatal connection marker 확인
2. recoverable timeout marker 확인
3. non-recoverable timeout marker 확인
4. cancel marker 확인
5. 일반 SQL error인지 확인
6. 판단 불가 시 UnsafeOrUnknown
```

단, 실제 구현에서 cancel marker와 fatal marker가 동시에 보이는 경우 fatal marker를 우선한다.

`has_connection_error`는 다음처럼 판단한다.

```rust
let has_connection_error =
    matches!(interrupt_kind, InterruptKind::ConnectionError | InterruptKind::NonRecoverableTimeout);
```

또는 기존 `error_allows_session_reuse` 함수를 유지하는 경우 다음처럼 사용한다.

```rust
let has_connection_error =
    !cancelled
    && !recoverable_timeout
    && !error_allows_session_reuse(err);
```

주의:

`error_allows_session_reuse(err) == false`를 바로 connection error로 보면 안 된다.

이유:

- cancel도 세션 재사용 불가 후보로 분류될 수 있다.
- timeout도 세션 재사용 불가 후보로 분류될 수 있다.
- 그러나 cancel, recoverable timeout은 별도 정책으로 살릴 수 있다.

---

## 11. Timeout 판단 규칙

`timed_out`은 elapsed time만으로 판단하지 않는다.

나쁜 예:

```rust
let timed_out = started_at.elapsed() >= query_timeout;
```

좋은 예:

```rust
let timed_out = is_timeout_error(err);
let recoverable_timeout = is_recoverable_timeout(err, db_type, sql_kind, lazy_state);
```

elapsed time은 로그나 진단 정보로만 사용한다.

최종 판단은 DB / driver가 반환한 error로 한다.

---

## 12. Recoverable timeout 정책

timeout도 살릴 수 있는 것은 살린다.

```rust
fn is_recoverable_timeout(
    db_type: DatabaseType,
    err_msg: &str,
    sql_kind: SqlKind,
    lazy_state: LazyFetchState,
) -> bool {
    if !sql_kind.is_select_like() {
        return false;
    }

    if matches!(lazy_state, LazyFetchState::Unknown) {
        return false;
    }

    let msg = err_msg.to_ascii_lowercase();

    match db_type {
        DatabaseType::Oracle => {
            err_msg.contains("DPI-1067")
        }

        DatabaseType::MySQL => {
            err_msg.contains("3024")
                || msg.contains("er_query_timeout")
                || msg.contains("maximum statement execution time exceeded")
                || msg.contains("max_execution_time")
        }

        DatabaseType::MariaDB => {
            msg.contains("max_statement_time")
                || msg.contains("maximum statement execution time exceeded")
        }
    }
}
```

다음 timeout은 recoverable로 보지 않는다.

```text
- socket timeout
- network timeout
- read timeout
- write timeout
- connection timeout
- Oracle ORA-3114
- Oracle ORA-03113
- Oracle ORA-03114
- MySQL error 2006
- MySQL error 2013
- server has gone away
- lost connection
- commands out of sync
```

### 12.1 Lock wait timeout

lock wait timeout은 connection 자체는 살아있을 수 있지만 transaction 상태가 애매할 수 있다.

따라서 자동 재사용 대상으로 보지 않는다.

권장 처리:

```text
DML + autocommit off + lock wait timeout
→ RequireCommitOrRollback 또는 MarkDirtyAndBlockNextExecution

그 외 lock wait timeout
→ 기본적으로 ReplacePhysicalSessionKeepUiConnected
```

---

## 13. Health check

물리 세션 재사용 직전 반드시 health check를 수행한다.

```sql
-- Oracle
SELECT 1 FROM dual;

-- MySQL / MariaDB
SELECT 1;
```

Oracle은 가능하면 `Connection::ping()`도 함께 사용할 수 있다.

권장 순서:

```text
Oracle:
1. timeout 설정 복구
2. ping 가능 시 ping
3. SELECT 1 FROM dual

MySQL / MariaDB:
1. timeout 설정 복구
2. ping 가능 시 ping
3. SELECT 1
```

health check 실패 시 같은 물리 세션을 재사용하지 않는다.

중요:

health check는 연결 생존 여부만 확인한다.

health check는 다음을 보장하지 않는다.

- transaction이 깨끗함
- lock이 없음
- package state가 안전함
- session variable이 초기값임
- script가 전부 rollback됨
- procedure side effect가 없음

따라서 health check만으로 재사용을 결정하면 안 된다.

---

## 14. 세션 재사용 조건

같은 물리 세션을 재사용하려면 다음 조건을 모두 만족해야 한다.

```text
1. SQL이 SELECT 계열이다.
2. DML / DDL / PL/SQL / script가 아니다.
3. 실행 worker가 종료됐다.
4. lazy fetch가 있다면 cursor close가 확인됐다.
5. lazy fetch fetching이었다면 fetch worker 종료가 확인됐다.
6. cancel 또는 recoverable timeout이다.
7. connection error가 아니다.
8. transaction dirty 가능성이 없다.
9. timeout 설정 복구가 성공했다.
10. health check가 성공했다.
11. operation_id와 connection_generation이 현재 상태와 일치한다.
```

operation_id 또는 connection_generation이 맞지 않는 stale event는 무시한다.
그 외 조건이 하나라도 실패하면 물리 세션을 폐기한다.

단, 물리 세션을 폐기해도 UI 논리 세션은 유지한다.

---

## 15. 세션 결정 enum

구현 시 다음 결정을 사용한다.

```rust
enum SessionDecision {
    IgnoreStaleEvent,
    ReuseSamePhysicalSession,
    ReplacePhysicalSessionKeepUiConnected,
    RequireCommitOrRollback,
    MarkDirtyAndBlockNextExecution,
}
```

### 15.1 IgnoreStaleEvent

`operation_id` 또는 `connection_generation`이 현재 상태와 맞지 않는 늦은 이벤트다.

현재 logical/physical session 상태를 변경하지 않는다.

### 15.2 ReuseSamePhysicalSession

같은 물리 세션을 재사용한다.

조건:

- SELECT 계열
- worker 종료
- cursor close 완료
- cancel 또는 recoverable timeout
- timeout 설정 복구 성공
- health check 성공

### 15.3 ReplacePhysicalSessionKeepUiConnected

현재 물리 세션은 폐기한다.

하지만 UI 논리 세션은 유지한다.

다음 쿼리 실행 시 새 물리 세션을 자동 획득한다.

### 15.4 RequireCommitOrRollback

미커밋 transaction 가능성이 있다.

사용자에게 다음 중 하나를 선택하게 한다.

- Commit
- Rollback
- 물리 세션 폐기

### 15.5 MarkDirtyAndBlockNextExecution

transaction 상태를 신뢰할 수 없고 자동 폐기도 위험한 경우 사용한다.

다음 SQL 실행 전에 사용자 결정을 요구한다.

---

## 16. 최종 decision 함수

cancel / timeout 후처리는 다음 의사코드에 맞춰 구현한다.

```rust
fn decide_session_after_interrupt(ctx: ExecContext) -> SessionDecision {
    // 1. stale event 방지
    if !ctx.operation_matches || !ctx.connection_generation_matches {
        return SessionDecision::IgnoreStaleEvent;
    }

    // 2. 연결 자체가 깨졌으면 같은 물리 세션 재사용 금지
    if ctx.has_connection_error {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    // 3. DDL/SessionControl interrupt는 rollback 가능성보다 물리 세션
    //    신뢰성이 더 애매하다. 이전에 보존해야 할 dirty/session-bound
    //    상태가 없으면 새 물리 세션으로 교체한다.
    if matches!(ctx.sql_kind, SqlKind::Ddl | SqlKind::SessionControl) {
        if ctx.prior_retained_state.requires_physical_session_preservation() {
            if ctx.prior_retained_state.may_have_uncommitted_work() {
                return SessionDecision::RequireCommitOrRollback;
            }
            return SessionDecision::RequirePhysicalSessionResolution;
        }
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    // 4. DML/PLSQL/script는 실행 범위와 transaction 상태가 애매함
    if matches!(ctx.sql_kind, SqlKind::Dml | SqlKind::PlsqlOrProcedure | SqlKind::Script) {
        if !ctx.autocommit {
            return SessionDecision::RequireCommitOrRollback;
        }

        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    // 5. SELECT 계열만 세션 재사용 후보
    if !ctx.sql_kind.is_select_like() {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    // 6. lazy fetch 상태 검증
    match ctx.lazy_state {
        LazyFetchState::None => {
            // 일반 SELECT
        }

        LazyFetchState::Waiting => {
            if !ctx.lazy_close_requested || !ctx.cursor_closed {
                return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
            }
        }

        LazyFetchState::Fetching => {
            if !ctx.lazy_cancel_requested || !ctx.fetch_worker_done || !ctx.cursor_closed {
                return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
            }
        }

        LazyFetchState::Closed => {
            // OK
        }

        LazyFetchState::CloseRequested
        | LazyFetchState::CancelRequested
        | LazyFetchState::Unknown => {
            return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
        }
    }

    // 7. timeout은 recoverable인 경우만 살림
    if ctx.timed_out && !ctx.recoverable_timeout {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    // 8. timeout 설정 복구 실패 시 세션 재사용 금지
    if !ctx.timeout_settings_restored {
        return SessionDecision::ReplacePhysicalSessionKeepUiConnected;
    }

    // 9. 최종 health check
    if ctx.health_check_ok {
        SessionDecision::ReuseSamePhysicalSession
    } else {
        SessionDecision::ReplacePhysicalSessionKeepUiConnected
    }
}
```

---

## 17. Cancel 처리 흐름

Cancel 버튼 클릭 시 흐름은 다음과 같다.

```text
1. cancel target snapshot 생성
2. cancel_flag = true
3. 일반 실행 worker 대상 cancel 요청
4. lazy fetch target 수집
5. lazy fetch waiting이면 GracefulClose 요청
6. lazy fetch fetching이면 CancelFetch 요청
7. worker 종료 이벤트 대기
8. cursor close 이벤트 대기
9. error kind 분류
10. timeout 설정 복구
11. health check
12. SessionDecision 결정
13. UI 상태 갱신
```

Cancel 요청은 비동기로 처리하되, 세션 재사용 결정은 worker 종료와 cursor close 확인 후에만 한다.

worker 종료 확인 전에는 같은 물리 세션으로 새 쿼리를 실행하면 안 된다.

---

## 18. Oracle cancel 처리

Oracle 일반 실행 cancel은 실행 중 connection에 대해 `break_execution()`을 사용한다.

처리 흐름:

```text
Cancel 클릭
→ current Oracle connection snapshot 획득
→ break_execution()
→ 실행 worker가 ORA-01013 또는 cancel 결과 수신
→ worker 종료 확인
→ timeout 설정 복구
→ SELECT 계열이면 health check
→ 성공 시 같은 물리 세션 재사용
→ 실패 시 물리 세션 교체
```

Oracle timeout 처리:

```text
DPI-1067
→ cleanup 성공 가능
→ SELECT 계열 + cursor closed + health check 성공 시 재사용 가능

ORA-3114 / ORA-03113 / ORA-03114
→ connection 신뢰 불가
→ 물리 세션 교체
```

---

## 19. MySQL / MariaDB cancel 처리

MySQL / MariaDB 일반 실행 cancel은 별도 cancel connection에서 `KILL QUERY <connection_id>`를 사용한다.

처리 흐름:

```text
Cancel 클릭
→ current connection_id snapshot 획득
→ 별도 connection으로 KILL QUERY 실행
→ 원 실행 worker가 error 1317 또는 interrupted 결과 수신
→ worker 종료 확인
→ timeout 설정 복구
→ SELECT 계열이면 health check
→ 성공 시 같은 물리 세션 재사용
→ 실패 시 물리 세션 교체
```

중요:

`KILL QUERY`는 connection을 즉시 폐기하지 않는다.

하지만 kill flag 확인과 cleanup에는 시간이 걸릴 수 있으므로, 원 실행 worker 종료 전에는 같은 물리 세션을 재사용하면 안 된다.

MySQL / MariaDB timeout 처리:

```text
error 3024 / ER_QUERY_TIMEOUT / max_execution_time
→ SELECT 계열이면 health check 후 재사용 가능

server has gone away / lost connection / commands out of sync / error 2006 / error 2013
→ 물리 세션 교체
```

---

## 20. Pooled session 처리

Cancel 또는 timeout 후 기존처럼 무조건 pooled session을 clear하지 않는다.

대신 다음 순서로 처리한다.

```text
1. interrupt 결과 수집
2. worker 종료 확인
3. cursor close 확인
4. timeout 설정 복구
5. error classification
6. health check
7. decision 결정
8. decision에 따라 reuse 또는 clear
```

나쁜 예:

```rust
if cancel_flag {
    pooled_session.clear();
}
```

좋은 예:

```rust
if cancel_flag || timed_out {
    let decision = decide_session_after_interrupt(ctx);
    apply_session_decision(decision);
}
```

---

## 21. Timeout 설정 복구

쿼리 실행 중 DB별 timeout 설정을 바꿨다면, 세션 재사용 전 반드시 원래 값으로 복구한다.

복구 실패 시 같은 물리 세션을 재사용하지 않는다.

```text
timeout setting restore 실패
→ 물리 세션 교체
```

Oracle:

- `set_call_timeout(previous_timeout)` 복구
- 복구 실패 시 세션 폐기

MySQL / MariaDB:

- session timeout 설정 복구
- 복구 실패 시 세션 폐기

---

## 22. Transaction 처리

DML / PL/SQL / script cancel 또는 timeout 후에는 트랜잭션 상태가 애매할 수 있다.

auto-commit off 상태에서는 자동 rollback하지 않는다.

이유:

- 사용자가 cancel 전에 같은 세션에서 수행한 미커밋 변경까지 함께 rollback될 수 있다.
- 일부 statement만 수행됐을 수 있다.
- procedure side effect를 알 수 없다.

따라서 다음 중 하나를 사용자에게 선택하게 한다.

```text
Commit
Rollback
Discard physical session
Cancel close / keep tab
```

auto-commit on 상태라도 DML / PL/SQL / script cancel 후 같은 물리 세션 재사용은 기본 금지한다.

---

## 23. UI 상태 표시

세션 처리 결과에 따른 UI 표시 원칙은 다음과 같다.

```text
같은 물리 세션 재사용:
Cancelled | Connected

물리 세션 교체 예정:
Cancelled | Connected

Commit/Rollback 필요:
Cancelled | Transaction decision required

새 물리 세션 획득 실패:
Disconnected

connection profile 자체가 제거됨:
Disconnected
```

물리 세션 폐기와 UI 연결 해제는 같은 의미가 아니다.

물리 세션을 폐기해도 논리 세션은 유지할 수 있다.

---

## 24. 다음 쿼리 실행 시 동작

Cancel 후 물리 세션이 교체 대상으로 표시된 상태에서 사용자가 다음 쿼리를 실행하면 다음 순서로 처리한다.

```text
1. 논리 세션의 연결 profile 확인
2. 기존 물리 세션이 reusable인지 확인
3. reusable이면 그대로 사용
4. replace_pending이면 새 물리 세션 획득
5. 새 물리 세션 획득 성공 시 실행
6. 획득 실패 시 UI를 Disconnected로 전환
```

---

## 25. 탭 닫기 처리

쿼리 실행 중인 탭을 닫으려는 경우 사용자가 다음 중 하나를 선택해야 한다.

- 실행 중인 쿼리를 취소하고 탭을 닫는다.
- 탭 닫기를 취소한다.

커밋이나 롤백이 필요한 탭을 닫으려는 경우 사용자가 다음 중 하나를 선택해야 한다.

- 커밋하고 탭을 닫는다.
- 롤백하고 탭을 닫는다.
- 물리 세션을 폐기하고 탭을 닫는다.
- 탭 닫기를 취소한다.

탭이 닫히면 관련 세션은 명시적으로 정리한다.

소유 탭이 없는 세션은 고아 세션으로 보고 정리한다.

---

## 26. 고아 세션 정리

다음 경우 고아 세션으로 본다.

- 탭이 닫혔는데 lazy fetch 세션이 남아 있음
- result tab이 닫혔는데 cursor가 남아 있음
- operation_id가 현재 탭 상태와 맞지 않음
- connection_generation이 현재 세션과 맞지 않음
- worker 완료 이벤트가 소유자를 찾지 못함

고아 세션은 재사용하지 않는다.

고아 세션은 가능한 한 close / cancel을 시도하고, 실패하면 pool에서 폐기한다.

---

## 27. 구현 변경 지침

### 27.1 Cancel 후 무조건 세션 폐기 금지

기존 로직에 다음 형태가 있다면 제거하거나 decision 기반으로 변경한다.

```rust
if cancel_flag {
    invalidate_or_clear_session();
}
```

변경 방향:

```rust
if cancel_flag {
    let decision = decide_session_after_interrupt(ctx);
    apply_session_decision(decision);
}
```

### 27.2 Lazy fetch command 분리

현재 cancel이 lazy fetch 세션을 바로 clear하는 구조라면, 다음처럼 분리한다.

```text
waiting 상태:
GracefulClose

fetching 상태:
CancelFetch

응답 없음:
ForceCancel + 물리 세션 교체
```

### 27.3 완료 이벤트 확장

`LazyFetchClosed` 이벤트는 가능하면 다음 정보를 포함하도록 확장한다.

```rust
struct LazyFetchClosedEvent {
    index: usize,
    session_id: u64,
    operation_id: u64,
    connection_generation: u64,
    cancelled: bool,
    cursor_closed: bool,
    fetch_worker_done: bool,
    error_kind: InterruptKind,
}
```

### 27.4 실행 완료 이벤트 확장

일반 실행 완료 이벤트도 세션 결정에 필요한 정보를 포함한다.

```rust
struct ExecutionFinishedEvent {
    tab_id: TabId,
    operation_id: u64,
    connection_generation: u64,
    db_type: DatabaseType,
    sql_kind: SqlKind,
    cancelled: bool,
    timed_out: bool,
    recoverable_timeout: bool,
    has_connection_error: bool,
    timeout_settings_restored: bool,
}
```

### 27.5 Health check 함수 추가

공통 health check 함수를 추가한다.

```rust
fn health_check_session(
    db_type: DatabaseType,
    session: &mut PhysicalSession,
) -> Result<bool, String> {
    match db_type {
        DatabaseType::Oracle => {
            // Prefer ping if available, then SELECT 1 FROM dual.
            // Return false on any error.
        }

        DatabaseType::MySQL | DatabaseType::MariaDB => {
            // Prefer ping if available, then SELECT 1.
            // Return false on any error.
        }
    }
}
```

### 27.6 Decision 적용 함수 추가

```rust
fn apply_session_decision(
    decision: SessionDecision,
    logical_session: &mut LogicalSession,
    physical_session: Option<&mut PhysicalSession>,
) {
    match decision {
        SessionDecision::IgnoreStaleEvent => {}

        SessionDecision::ReuseSamePhysicalSession => {
            logical_session.mark_connected();
            logical_session.clear_replace_pending();
        }

        SessionDecision::ReplacePhysicalSessionKeepUiConnected => {
            if let Some(session) = physical_session {
                session.discard();
            }

            logical_session.mark_connected();
            logical_session.mark_replace_pending();
        }

        SessionDecision::RequireCommitOrRollback => {
            logical_session.mark_connected();
            logical_session.mark_transaction_decision_required();
        }

        SessionDecision::MarkDirtyAndBlockNextExecution => {
            logical_session.mark_connected();
            logical_session.mark_dirty();
            logical_session.block_next_execution();
        }
    }
}
```

---

## 28. Acceptance Criteria

### 28.1 일반 SELECT cancel

```text
SELECT 장시간 실행
→ Cancel
→ worker 종료
→ health check 성공
→ UI: Cancelled | Connected
→ 다음 SELECT 실행 성공
→ 가능한 경우 같은 물리 세션 재사용
```

### 28.2 lazy fetch waiting cancel

```text
SELECT 결과 일부 fetch
→ lazy fetch waiting
→ Cancel
→ GracefulClose
→ LazyFetchClosed(cursor_closed = true)
→ health check 성공
→ 같은 물리 세션 재사용
```

### 28.3 lazy fetch fetching cancel

```text
fetching 중 Cancel
→ CancelFetch
→ worker 종료
→ cursor close 확인
→ health check 성공
→ 같은 물리 세션 재사용
```

단, worker 종료 또는 cursor close 확인 실패 시 물리 세션 교체.

### 28.4 Oracle recoverable timeout

```text
Oracle SELECT timeout
→ DPI-1067
→ worker 종료
→ health check 성공
→ 같은 물리 세션 재사용
```

### 28.5 Oracle non-recoverable timeout

```text
Oracle timeout
→ ORA-3114 또는 ORA-03114
→ UI: Connected 유지
→ 물리 세션 교체
```

### 28.6 MySQL recoverable timeout

```text
MySQL SELECT timeout
→ error 3024 / ER_QUERY_TIMEOUT
→ worker 종료
→ health check 성공
→ 같은 물리 세션 재사용
```

### 28.7 MySQL connection error

```text
MySQL 실행 중 lost connection / server has gone away
→ UI는 우선 Connected 유지 가능
→ 물리 세션 교체
→ 다음 실행에서 새 세션 획득 실패 시 Disconnected
```

### 28.8 DML cancel with autocommit off

```text
UPDATE 실행 중 Cancel
→ transaction 상태 불확실
→ Commit/Rollback/Discard 선택 요구
→ 자동 rollback 금지
```

### 28.9 Script cancel

```text
script 실행 중 Cancel
→ 일부 statement 실행 가능성 있음
→ 같은 물리 세션 재사용 금지
→ 필요 시 dirty-session 처리
```

### 28.10 다중 탭

```text
탭 A 실행 중
탭 B 실행 중
탭 A cancel
→ 탭 B 세션 영향 없음
```

### 28.11 stale event

```text
이전 operation의 LazyFetchClosed 이벤트가 늦게 도착
→ operation_id 불일치
→ 현재 세션 상태 변경하지 않음
```

---

## 29. 테스트 케이스

### 29.1 Oracle

- 장시간 SELECT cancel 후 `SELECT 1 FROM dual`
- lazy fetch waiting 상태 cancel 후 다음 SELECT
- lazy fetch fetching 상태 cancel 후 다음 SELECT
- `DPI-1067` timeout 후 health check
- `ORA-3114` timeout 후 물리 세션 교체
- PL/SQL block cancel 후 dirty 처리
- UPDATE cancel with autocommit off 후 Commit / Rollback 선택 요구

### 29.2 MySQL / MariaDB

- `SELECT SLEEP(30)` cancel 후 `SELECT 1`
- `KILL QUERY` 후 worker 종료 전 재사용하지 않는지 확인
- error 3024 timeout 후 health check
- `server has gone away` 후 물리 세션 교체
- lazy fetch waiting close 후 재사용
- lazy fetch fetching cancel 후 재사용
- UPDATE / DELETE cancel with autocommit off 후 transaction decision 요구

### 29.3 공통

- 다중 탭 cancel 대상 격리
- result tab 닫힘 후 lazy fetch 고아 세션 정리
- health check 실패 시 물리 세션 교체
- 새 물리 세션 획득 실패 시에만 Disconnected 표시
- cancel 직후 UI가 Connected 유지되는지 확인
- operation_id 불일치 이벤트 무시
- connection_generation 불일치 이벤트 무시

---

## 30. 한 줄 원칙

Cancel, lazy fetch, timeout 이후에도 논리 세션은 유지한다.

물리 세션은 worker 종료, cursor close, recoverable error, transaction 안전성, timeout 설정 복구, health check가 모두 확인된 경우에만 재사용한다.
