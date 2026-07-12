# Result UI 현재 동작

> 구현 대조: 2026-07-12 (`src/ui/result_tabs.rs`, `src/ui/main_window.rs`)

## 목적

결과 영역은 표 형식 결과와 보조 출력이 서로의 상태를 덮어쓰지 않도록 분리한다.
표 형식 결과는 statement별 수명 주기와 lazy fetch 세션을 소유하고, 스크립트
출력·DBMS output·일반 메시지는 고정된 보조 pane에 누적한다.

## 상위 탭

현재 고정 상위 탭은 다음 4개이며 이 순서를 유지한다.

1. Data Grid
2. Script Output
3. DBMS Output
4. Messages

Explain Plan은 별도 상위 탭이 아니다. 실행 계획 텍스트는 Data Grid 아래에
`Explain Plan`이라는 statement result tab으로 추가된다.

## Data Grid

Data Grid는 `ResultTabId`로 식별되는 statement result tab 목록을 가진다. 각 탭은
다음 상태 중 하나를 유지한다.

- `Running`
- `Fetching`
- `Waiting`
- `Canceling`
- `Done`
- `Error`
- `Cancelled`

탭 제목에는 statement label, 상태, 필요한 경우 row count가 반영된다. 일반
SELECT, DML RETURNING, ref cursor, Quick Describe, object-browser 결과와 Explain
Plan이 이 영역을 사용한다.

Data Grid result tab만 다음 기능을 가진다.

- lazy fetch 세션 연결과 추가 fetch/fetch all/cancel
- Oracle `ROWID` 기반 편집
- 선택 영역 복사와 header 포함 복사
- 편집 가능한 grid 붙여넣기
- CSV 내보내기
- result tab 닫기

`active_result_id()`는 Data Grid가 현재 상위 탭이고 실제 내부 result tab이
선택된 경우에만 `Some(ResultTabId)`를 반환한다. Script Output, DBMS Output,
Messages에서는 `None`이다. 편집·내보내기·닫기 동작은 이 규칙을 사용해 숨겨진
grid에 잘못 적용되는 것을 막는다.

## Script Output

현재 내부 탭은 다음 2개다.

- Output
- Errors

`QueryProgress::ScriptOutput`은 `append_script_output_lines()`를 통해 Output에
누적된다. 실행 중 선택할 Data Grid가 아직 없다면 Script Output이 선택될 수
있다. Output buffer는 무한히 커지지 않도록 구현에서 길이를 제한한다.

Errors pane은 UI 구조에는 존재하지만 현재 별도 append API나 자동 routing이
없다. SQL 실행 오류는 Messages > Errors와 해당 Data Grid 상태로 전달된다.
Script Output 내부 탭 자체는 닫을 수 없으며, 상위 지원 영역에 대한 clear
동작은 Output과 Errors를 함께 비운다.

## DBMS Output

Oracle `DBMS_OUTPUT` line은 `append_dbms_output_lines()`를 통해 이 pane에
누적된다. DBMS output만 생성되고 선택된 Data Grid가 없다면 이 pane을 선택할
수 있다. 표 형식 결과와 함께 생성된 경우에는 Data Grid를 사용자의 주 시각적
anchor로 유지할 수 있다.

## Messages

현재 내부 탭과 `ResultMessageKind`는 다음 2종류다.

- Info
- Errors

Warning 전용 kind와 pane은 아직 없다. 일반 실행 정보는 Info에, SQL/연결 오류는
Errors에 누적한다. SQL 실패 시 message만 남기는 것이 아니라 연결된 Data Grid
result tab도 `Error` 또는 `Cancelled` 상태로 끝내야 한다. 이렇게 해야 progress,
lazy fetch, tab close 상태가 statement와 계속 연결된다.

지원 pane clear는 현재 선택된 Messages 상위 영역의 Info와 Errors를 함께 비운다.

## Explain Plan

`QueryProgress::ExplainPlanOutput`은 `append_explain_plan_tab()`으로 전달된다. 이 함수는
새 `ResultTabId`를 예약하고 Data Grid 아래에 `Explain Plan` result tab을 만든 뒤,
계획의 각 줄을 `Text` 단일 컬럼 row로 표시한다. 성공 시 해당 Data Grid tab이
선택된다. 실패는 일반 오류 routing을 따라 Messages > Errors로 전달된다.

## 선택 규칙

- 표 형식 결과가 시작되면 연결된 Data Grid result tab을 선택한다.
- Script Output/DBMS Output/Info는 현재 실행에 선택된 grid가 없을 때 보조 pane을
  선택할 수 있다.
- Error message는 Messages > Errors를 선택한다.
- 일반 Info message 때문에 이미 표시 중인 Data Grid를 강제로 빼앗지 않는다.
- Explain Plan 성공은 새 Data Grid result tab을 선택한다.

실제 선택 여부는 `main_window.rs`의 progress context와
`should_select_support_result_pane()` 정책이 결정한다.

## 닫기와 정리

- 사용자가 닫을 수 있는 것은 Data Grid의 개별 result tab뿐이다.
- 닫는 result tab에 lazy fetch 세션이 연결돼 있으면 session id를 회수해 정리
  경로로 전달한다.
- Script Output/DBMS Output/Messages 상위 영역은 닫지 않고 clear한다.
- 전체 clear는 모든 Data Grid와 모든 보조 pane을 비운다.

`ResultTabCloseTarget::ScriptOutput` variant와
`close_script_output_tab()`/`close_current_script_output_tab()`은 현재 호환용
표면으로 남아 있지만 close 함수는 `false`를 반환하는 no-op이다.

## 주요 구현 API

- `reserve_result_tab_id()`
- `ensure_statement_tab_by_id()`
- `display_result_by_id()`
- `active_result_id()`
- `append_script_output_lines()`
- `append_dbms_output_lines()`
- `append_message_lines()`
- `append_explain_plan_tab()`
- `clear_current_support_section()`
- `close_tab_by_id_and_take_lazy_fetch()`

## 검증

```sh
cargo test result_tabs --lib
cargo test main_window --lib
cargo test --test ui_dialog_guards
cargo check --bin space_query
```

중점 확인 사항:

- 상위 탭 순서가 4개로 고정되는가
- Data Grid 밖에서 `active_result_id()`가 `None`인가
- statement별 상태와 row count가 올바른 tab에 반영되는가
- SQL 오류가 Messages와 Data Grid 상태 양쪽에 반영되는가
- Explain Plan이 Data Grid result tab으로 만들어지는가
- 보조 pane에서는 grid 편집·내보내기·닫기가 비활성화되는가
