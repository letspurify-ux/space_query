# SPACE Query 다중 DB 워크스페이스 구현 계획

## 목표와 확정 동작

현재 FLTK UI의 단일 `SharedConnection`, 전역 메타데이터와 전역 결과 영역을 연결 레지스트리와 탭별 워크스페이스로 교체한다. DataGrip의 콘솔-데이터소스/세션 모델과 Toad의 열린 연결 및 선택된 Editor Window 단위 트랜잭션 모델을 참고한다.

- 여러 Oracle/MySQL/MariaDB 연결을 동시에 열고 실행한다.
- 쿼리 탭은 생성 시 연결에 고정되며, 명시적 스크립트 `CONNECT`/`DISCONNECT`만 현재 탭 바인딩을 변경한다.
- 같은 저장 프로필을 다시 열면 기존 런타임과 풀을 재사용하고 쿼리 탭만 추가한다.
- 서로 다른 탭은 같은 연결 또는 다른 연결에서 실제 병렬 실행한다.
- 현재 스키마/DB 선택은 `ConnectionId` 단위로 공유하고, SQL*Plus 상태, 바인드, delimiter, retained session과 결과는 탭별로 독립한다.
- Auto-Commit과 기본 isolation/access는 연결별 기본값이고 스크립트 `SET` 상태만 탭별 override로 둔다.
- 한 SQL 문장은 하나의 DB에서만 실행한다. 한 스크립트는 명시적 `CONNECT`로 문장 사이에서 DB를 순차 전환할 수 있다.
- 재바인딩, 스코프 변경 또는 재연결 전 결과는 보존하되 읽기 전용으로 전환한다.
- 교차 DB JOIN, 결과 병합과 분산 트랜잭션은 범위에서 제외한다.

## 상태 소유권과 인터페이스

### 연결

- `ConnectionId(u64)`는 앱 실행 동안 유일하며 영속화하지 않는다.
- `ConnectionRegistry`는 `ConnectionId -> Arc<ConnectionRuntime>`과 저장 프로필 인덱스를 관리한다.
- `ConnectionRuntime`은 `SharedConnection`, 풀, sanitized 연결 정보, reconnect 정보, 연결별 Auto-Commit/transaction 기본값과 상태를 소유한다.
- 연결 상태는 `Connecting`, `Connected`, `Transitioning`, `Disconnected`, `Failed`를 구분한다.
- 저장 프로필 runtime은 명시적 disconnect 또는 앱 종료까지 유지한다. 스크립트 transient runtime은 바인딩된 탭과 활성 작업이 없어지면 자동 정리한다.
- 비밀번호는 기존 keyring/`DatabaseConnection` 경계 밖으로 노출하지 않는다. event, registry, activity, history, status와 log에는 넣지 않는다.

### 탭

- `TabWorkspace`가 에디터, 연결 binding, 탭 전용 `SessionState`, retained physical session, 실행 상태, IntelliSense와 `ResultWorkspace`를 소유한다.
- binding은 `Bound`, `Detached`, `Unbound`와 `binding_revision`을 가진다. 선택 scope는 같은 `ConnectionId`의 모든 binding에 동기화한다.
- UI Disconnect는 binding을 유지한 채 runtime을 offline으로 만든다. 스크립트 `DISCONNECT`는 현재 탭만 detach한다.
- FLTK 실행 경로는 `DatabaseConnection::session_state()`를 사용하지 않고 탭 전용 상태를 명시적으로 전달한다. 저수준 `SharedConnection` 타입과 TUI 호환 경로는 유지한다.

### 비동기 이벤트

- `QueryOperationToken`은 `{ tab_id, editor_id, operation_id }`를 식별한다.
- 각 DB 문장은 `ExecutionOrigin { connection_id, connection_generation, pool_context_epoch, binding_revision, db_type, scope, display_name }`을 캡처한다.
- 결과 이벤트는 operation token으로 라우팅하고 과거 origin도 표시한다. binding/scope/IntelliSense 변경만 revision 일치를 요구한다.
- metadata 이벤트는 `{ connection_id, connection_generation, scope, request_id }`가 모두 일치할 때만 반영한다.
- 결과는 `ResultAddress { query_tab_id, result_tab_id }`로 식별한다.
- `ResultTabRequest`, lazy fetch, 결과 편집과 객체 액션은 문자열/활성 탭 추론 대신 typed origin/target을 전달한다.

### 명시적 스코프

- FLTK 경로는 공유 `DatabaseConnection`의 global current database/schema를 바꾸지 않는다.
- 풀 API에 `acquire_session_for_scope(scope)`를 추가하고 탭 및 metadata 작업이 항상 scope를 전달한다.
- `USE`, Oracle current-schema 변경과 scope selector는 같은 `ConnectionId`의 모든 탭 binding, metadata와 적용 가능한 retained session에 반영한다. Oracle/MySQL/MariaDB에 같은 정책을 사용한다.
- AppState/registry 잠금 중 네트워크 또는 `DatabaseConnection` 잠금을 잡지 않으며 FLTK 위젯은 메인 스레드에서만 변경한다.

## 구현 변경

### 연결 수명주기

- `AppState.connection`, 전역 `connection_info`, `has_live_connection`을 registry와 활성 탭 binding 조회로 교체한다.
- 같은 저장 프로필이 Connected이면 handshake 없이 탭만 추가하고, Connecting이면 연결 시도를 합치며, offline이면 같은 `ConnectionId`를 reconnect한다.
- 열린 프로필 설정 변경은 다음 reconnect부터 적용한다.
- 스크립트 `CONNECT user/password@...`는 항상 새로 인증한다. 성공하기 전까지 기존 binding/session을 보존하고, dirty/locked retained session 또는 lazy fetch가 있으면 거부한다.
- CONNECT 성공 시 compare-and-swap으로 현재 탭 binding을 transient runtime으로 교체하고 세션/override를 초기화한다. 실패, 취소, stale operation이면 후보 연결을 폐기한다.
- `Disconnect Active Connection`은 그 runtime의 탭만 offline으로 만들고, 스크립트 `DISCONNECT`는 현재 탭만 detach한다.
- saved runtime reconnect는 keyring을 사용한다. transient reconnect는 sanitized 정보를 prefill하고 비밀번호를 다시 입력받는다.
- reconnect endpoint가 달라졌으면 metadata cache를 버린다. 기존 scope가 유효하지 않으면 조용히 fallback하지 않고 재선택을 요구한다.

### 탭별 병렬 실행과 결과

- 전역 active editor/result alias를 실행 라우팅에 사용하지 않고 모든 callback이 생성 시 캡처한 `QueryTabId`를 사용한다.
- `is_any_query_running()` 실행 차단을 제거하고 동일 탭 중복 실행만 차단한다.
- 같은 runtime의 여러 탭은 공유 풀 한도 내에서 병렬 실행한다.
- 풀 부족 시 같은 `ConnectionId`의 가장 오래된 lazy fetch만 회수 후보로 삼는다.
- Cancel, timeout과 force-cancel은 operation token과 origin이 일치하는 세션만 대상으로 한다.
- 각 탭은 Data Grid, Script Output, DBMS Output, Messages를 포함한 자체 결과 workspace를 가진다. 하단 toolbar는 active workspace를 조작하는 stateless controller로 유지한다.
- background tab 결과는 해당 workspace에 추가하며 focus나 활성 result를 훔치지 않는다.
- 결과 제목/tooltip에 `연결 · Result N`을 표시하고 스키마/계정은 노출하지 않는다.
- 결과 편집은 소유 탭/result 주소, connected runtime, connection generation, binding revision/scope와 탭 idle 상태가 모두 일치할 때만 허용한다.
- 재연결/재바인딩/scope 변경 전 결과는 read/export만 허용하고 edit/save/insert/delete/lazy-fetch를 막는다. 활성 DB로 fallback하지 않는다.

### 트랜잭션, 설정과 병렬 UI

- Auto-Commit과 기본 isolation/access는 runtime 단위다. 탭의 스크립트 override는 runtime 기본값 변경 후에도 유지한다.
- Auto-Commit/transaction mode 변경은 같은 `ConnectionId` 탭만 preflight하고 retained session을 갱신한다.
- Commit/Rollback은 활성 탭 retained session만 대상으로 한다.
- runtime disconnect는 대상 runtime의 실행/lazy fetch가 끝난 후 관련 탭의 retained session만 commit/rollback/discard 처리한다.
- pool size는 모든 runtime의 목표값이다. 전체 preflight와 transition 예약 후 runtime별 resize를 수행하고 부분 실패 시 실패 runtime의 기존 풀을 보존한다.
- substitution prompt는 operation/tab/connection별 요청을 한 번에 하나씩 표시하는 broker로 직렬화한다.
- 비동기 실행에는 global Wait cursor를 쓰지 않고 탭 badge와 상태 표시줄을 사용한다.
- SPOOL은 탭별 상태로 두되 동일 정규화 경로의 동시 소유를 거부하고 `SPOOL OFF`, 연결 전환, 탭 종료 때 해제한다.
- Query History와 Session Activity에 실제 connection/scope origin을 기록한다.

### 오브젝트 브라우저와 metadata

- 상단 연결 콤보에서 runtime을 선택하고 runtime마다 독립적인 tree, metadata cache와 worker 수명을 유지한다.
- 연결 선택 콤보는 하단 scope 콤보와 동일한 폭, 좌우 여백과 높이를 사용한다.
- scope, category, package 등 object 하위 멤버는 선택된 runtime의 tree에서 lazy load한다.
- scope selector는 `ConnectionId` 단위이며 변경 시 같은 연결을 사용하는 모든 쿼리 탭에 반영한다.
- tree/연결 콤보 선택만으로 현재 탭을 재바인딩하지 않는다. `New SQL File`, `Open SQL File`, Recent File 및 마지막 탭 자동 생성은 연결 콤보에서 선택한 runtime을 사용한다.
- `TreeItem::as_ptr()` side map에 `{ tree_epoch, connection_id, scope, kind, name }`을 저장하고 라벨을 역파싱하지 않는다.
- node는 Unloaded/Loading/Loaded/Error 상태를 가지며 중복 요청을 합친다. filter는 이미 로드된 node만 대상으로 한다.
- metadata/IntelliSense cache는 `(ConnectionId, ScopeKey)`로 분리한다. DDL 성공 시 정확한 scope를 invalidate하고 판별 불가능하면 그 connection 전체를 invalidate한다.
- offline runtime은 마지막 cache를 stale 표시하고 DB 액션을 비활성화한다.
- 다른 연결의 객체 액션은 source 연결 탭을 생성/사용하며 cross-connection drag/drop은 거부한다. 같은 연결의 다른 scope 객체는 완전 수식 이름으로 삽입한다.

### UI와 적용 순서

- 앱 시작 시 합성 `ORCL` runtime을 만들지 않고 연결 콤보와 최초 쿼리 탭을 unbound 상태로 둔다.
- 쿼리와 결과 탭 header는 `연결 · 문서/Result` 순서로 표시하며 스키마/계정은 표시하지 않는다. running, offline, transaction decision과 override 상태는 별도 상태 표시에 유지한다.
- `Tools > Session Activity`는 모든 runtime을 ConnectionId/상태/scope/tab/result별로 표시한다.
- DB activity에 ConnectionId를 추가해 한 연결 disconnect가 다른 연결 활동을 지우지 않게 한다.
- 메뉴는 Connect, Reconnect Active Connection, Disconnect Active Connection, Disconnect All을 제공하고 `Ctrl+D`는 active runtime만 대상으로 한다.

1. 단일 연결 characterization test와 plan 문서를 고정한다.
2. ConnectionId/registry/runtime과 typed event context를 단일 runtime 호환 경로에 도입한다.
3. 탭별 binding/session/result/progress ownership으로 이동한다.
4. explicit-scope pool API와 connection/scope metadata cache를 적용한다.
5. 전역 실행 차단을 탭/runtime 단위로 좁혀 병렬 실행을 활성화한다.
6. 다중 root browser와 탭 connection/scope UI를 연결한다.
7. script CONNECT/DISCONNECT, transient lifecycle, prompt/SPOOL 병렬 안전성을 적용한다.
8. 설정, Session Activity, 종료 처리, 문서와 screenshot을 갱신한다.

## 검증 및 완료 기준

- registry ID 유일성, saved profile reuse, connecting coalescing, transient cleanup, reconnect/실패 상태와 password redaction을 단위 테스트한다.
- 같은 runtime 두 탭의 scope 동기화와 SessionState, bind, delimiter, retained session 및 결과 격리를 검증한다.
- 다른 DB와 같은 runtime의 장기 쿼리를 동시에 실행하고 한쪽 cancel/timeout/connection loss가 다른 쪽에 영향 없는지 검증한다.
- ConnectionId 충돌, stale generation/binding/request, 탭 종료 후 늦은 event를 무시하는지 검증한다.
- script CONNECT 전후 statement result/history origin과 실패 시 기존 binding 보존을 검증한다.
- background result 격리, active-workspace clear/export/close, stale result read-only와 grid edit no-fallback을 검증한다.
- UI Disconnect와 script DISCONNECT 범위, reconnect cache/scope 정책을 검증한다.
- 동일 이름 object가 여러 root에 있을 때 refresh/context action/drag가 정확한 connection으로 라우팅되는지 검증한다.
- prompt 직렬화, 동일 SPOOL path 충돌과 cleanup을 검증한다.
- commit/rollback 독립성, targeted disconnect와 app exit의 retained-state resolution을 검증한다.
- `cargo fmt --check`, `cargo test --lib`, `cargo test --tests`, `cargo clippy --all-targets -- -D warnings`를 통과한다.
- 100%/150% UI scale 및 1200x800에서 다중 root, 긴 연결명, 상태 badge와 workspace 전환을 수동 확인한다.

## 제외 범위

- FLTK desktop UI만 대상으로 하며 TUI 다중 연결은 제외한다.
- 열린 연결, 탭, scope와 결과를 앱 재시작 후 복원하지 않는다.
- 저장 connection 설정과 `last_connection` config 형식은 유지한다.
- 연결 하나당 pool 하나를 공유하고 탭마다 retained physical session 하나를 소유한다.
- connection 색상 설정, 임의 연결 수 제한, client-side federation, cross-DB JOIN과 분산 transaction은 추가하지 않는다.
