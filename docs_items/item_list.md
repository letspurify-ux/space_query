# DataGrip 대비 미구현 기능 목록 (시급도 순)

작성일: 2026-08-06 · 갱신일: 2026-08-08 (항목 4·11 완료 기록, 항목 28 재검증 기록. 그 전 갱신: 항목 31 완료 기록, 항목 10·12·28 완료 기록, 항목 7·8 완료 기록, 구현 상태 재확인, 항목 24~33 추가)

## 조사 범위와 기준

- **대상**: DataGrip에서 *일반 사용자가 일상적으로* 쓰는 기능. DBA 전용/고급 튜닝
  기능이나 팀 협업(VCS 연동) 기능은 뒤로 뺐다.
- **판정 근거**: SPACE Query 실제 소스에서 해당 기능의 진입점(메뉴/컨텍스트 메뉴/
  단축키/핸들러)이 존재하는지 확인. 각 항목에 확인 지점을 적었다.
- **제외**: 이미 구현된 것 — 결과 편집(staged edit), CSV 복사/내보내기, SQL
  Inserts/Updates/Where Clause 클립보드 내보내기, IntelliSense, 시그니처 힌트,
  포매터, 스크립트 실행, 객체 트리 + Generate DDL, 테이블 브라우징 페이징,
  쿼리 히스토리, 세션 액티비티, 애플리케이션 로그, 탭별 커넥션 바인딩,
  트랜잭션 격리 수준 설정.
- **제외 추가 (2026-08-08 재조사)**: 그리드 행 삽입/삭제와 `Set Null`
  (`result_table.rs:6206`, `7044`, 컨텍스트 메뉴 `result_table.rs:7279`),
  SQL*Plus 치환 변수 `&var` 값 프롬프트(`execution.rs:19331`)와 `VARIABLE`
  바인드 선언(`ToolCommand::Var`, `db/query/types.rs:251`), 커넥션 SSL/TCPS 설정
  (`connection_dialog.rs:1131`, `1293`), 커넥션별 기본 트랜잭션 접근 모드
  (READ ONLY 포함, `connection.rs:582`), 스코프(스키마) 선택기, 쿼리 타임아웃
  설정. DataGrip 대비 구멍으로 적었을 뻔한 것들이라 여기 남긴다.

시급도 판정 기준: **(사용 빈도) x (없을 때의 우회 비용) x (데이터 안전성 영향)**.

---

## Tier 0 — 시급. 매일 쓰는 동작인데 우회로가 없음

### 1. 일반 쿼리 결과의 WHERE / ORDER BY 필터 (서버 재조회)

- **DataGrip**: 그리드 상단 필터 입력줄에 `WHERE` 조각을 넣으면 서버에 재조회.
- **현재 상태**: `WHERE`/`ORDER BY` 필터 바는 **테이블 브라우징 탭에만** 있다
  (`src/ui/table_browse.rs`). 일반 쿼리 결과 탭에는 없다. 그리드 안 텍스트 검색도
  없다 — `src/ui/result_table.rs`에 `Ctrl+F` 핸들러가 없고, `FindReplaceDialog`는
  SQL 에디터에만 연결되어 있다(`src/ui/main_window.rs:8664`, `10900`).
- **왜 시급한가**: 1,000행 결과에서 조건 하나 바꾸려고 매번 에디터로 돌아가
  쿼리를 고쳐 다시 실행해야 한다. 그리드를 "보는 것"에서 "쓰는 것"으로 바꾸는
  최소 기능.

**결정: 로컬 평가 방식이 아니라 서버 재조회 방식으로 간다.**

로컬(이미 fetch된 행) 필터도 검토했다. 그리드에 `column_kinds`
(`result_table.rs:285`)가 이미 있어서 타입별 비교가 기술적으로 가능하고
DB 없이 테스트할 수 있다는 장점이 있었다. 하지만 채택하지 않았다. 값이 전부
문자열(`QueryResult.rows: Vec<Vec<String>>`)이라 NULL 판정이 휴리스틱
(`value_represents_null`, `result_table.rs:601`)이고 날짜 역파싱이 세션 NLS
포맷에 묶이는 데다, 무엇보다 **SQL과 미묘하게 다른 평가기를 영구히 유지보수하는
비용**(대소문자/collation, 3값 논리, 암묵 형변환, 함수 지원 범위)이 가장 컸다.
서버 재조회는 이 문제를 통째로 없앤다 — 의미론이 정의상 정확하다.

구체적 설계·게이트·검증 계획은 문서 끝의
[부록 A](#부록-a-일반-쿼리-결과-필터-구현-계획)에 있다.

- **난이도**: 중. `table_browse`의 SQL 조립·페이징 파이프라인이 거의 그대로
  재사용된다(`relation_sql`이 테이블명이 아니라 자유 관계식이라서). 새로 드는
  비용은 파이프라인이 아니라 **게이트와 상태 전환**이다.

### 2. CSV 이외의 결과 내보내기 포맷

- **DataGrip**: Export Data → CSV/TSV/JSON/XML/HTML/Markdown/Excel/SQL Insert
  스크립트, 그리고 "파일로" 또는 "클립보드로" 선택.
- **현재 상태**: 파일 내보내기 경로는 CSV 하나뿐이다
  (`ResultTabs::export_to_csv`, `export_to_csv_after_fetch_all` —
  `src/ui/result_tabs.rs:2561`). 컨텍스트 메뉴의 SQL Inserts/Updates/Where Clause는
  **클립보드 전용**이고 선택 영역만 대상이다
  (`src/ui/result_table.rs:7430` 부근). JSON/XML/Excel 관련 코드 없음.
- **왜 시급한가**: "결과를 JSON으로 뽑아서 API 목업에 넣기", "결과 전체를 INSERT
  스크립트 파일로 저장해 다른 환경에 반영하기"는 일상 작업이다. 현재는 CSV로
  뽑아 외부 도구로 변환해야 한다.
- **난이도**: 소~중. 이미 그리드 값 → 타입별 SQL 리터럴 변환 로직이
  `src/ui/grid_sql_export.rs`에 있으므로, **"선택 영역 → 클립보드" 파이프라인을
  "전체 결과 → 파일"로 확장**하면 SQL Insert 파일 내보내기는 거의 공짜다.
  JSON/TSV/Markdown도 직렬화기 추가 수준. Excel(xlsx)만 별도 크레이트 필요.

#### 구현 상태 (2026-08-07) — 완료 (Excel 제외)

`Ctrl+E` / **Tools > Export Results** / 그리드 컨텍스트 메뉴 **Export Results**가
모두 같은 모달을 연다: **포맷**(CSV/TSV/JSON/XML/HTML/Markdown/SQL Inserts) ×
**행 범위**(전체 / 선택 영역) × **대상**(파일 / 클립보드).

| 조각 | 위치 |
| --- | --- |
| 직렬화기 (순수 함수, 테스트 19개) | `src/ui/result_export.rs` |
| 모달 | `src/ui/result_export_dialog.rs` |
| 스냅샷 + 지연 렌더 (`ExportRequest`) | `ResultTableWidget::export_grid_snapshot` / `export_after_fetch_all` |
| 진입점 | `MainWindow::export_current_results` |

설계상 결정 세 가지:

- **`SQL Inserts`만 `grid_sql_export`가 렌더한다.** 방언에 의존하므로 커넥션이
  없으면 포맷 목록에서 아예 빠진다. `result_export::render`는 이 포맷에 대해
  의도적으로 빈 문자열을 돌려주므로, 라우팅 실수가 *틀린 SQL*이 아니라
  *아무것도 아닌 것*이 된다(테스트로 고정).
- **컬럼 범위가 포맷별로 다르다.** 데이터 포맷은 화면에 보이는 것을 그대로
  내보내고 숨은 auto-`ROWID`만 뺀다. `SQL Inserts`는 기존의 엄격한 내부 컬럼
  규칙을 유지한다 — 생성된 SQL은 합법적인 컬럼명이 필요하기 때문.
- **NULL은 포맷의 어휘를 따른다.** CSV/TSV는 그리드의 NULL 표시 텍스트를 그대로
  쓰고(스프레드시트 덤프), JSON은 `null`, XML은 빈 엘리먼트, HTML/Markdown은 빈
  셀. JSON에서 값이 따옴표 없이 나가는 건 드라이버가 `Number`/`Boolean`으로
  타이핑했고 **동시에** 그 텍스트가 이미 유효한 JSON 리터럴일 때뿐이다
  (`00123`, `.5`는 문자열로, `1.2E+10`은 숫자로).

**전체 행** 내보내기는 열려 있는 lazy fetch를 먼저 끝낸다
(`LazyFetchPendingAction::Export`). **선택 영역**은 화면에 이미 있는 행만
대상이므로 절대 fetch를 유발하지 않는다.

부수적으로 CSV 경로가 새 직렬화기 위로 합쳐졌다 — `build_csv_snapshot`이
`result_export::render(Csv, ..)`를 호출하므로 CSV 구현이 하나가 됐고, BOM·플랫폼
줄바꿈·이스케이프 규칙은 바이트 단위로 그대로다.

**형식 검증 — 실제 파서로 확인.** 유닛 테스트는 "내가 기대한 바이트"만 고정하지
"JSON 파서가 받아주는가"는 증명하지 못한다. `src/bin/verify_result_export.rs`가
적대적 그리드 하나(쉼표·탭·따옴표·개행·단독 CR·`|`·`\`·`]]>`·C0 제어문자·한글·
NULL·빈 문자열·0 패딩 숫자, 그리고 비어 있거나/중복이거나/구두점이 들어가거나/
숫자로 시작하는 컬럼명)를 전 포맷으로 렌더해 **실제 파서**에 넣고 셀 단위로
되돌려 비교한다 — JSON은 `serde_json`, XML은 `xmllint` + `ElementTree`,
HTML은 파이썬 `html.parser`, CSV/TSV는 파이썬 `csv`(excel 방언).

이 검증이 **실제 버그 하나를 잡았다**: 값에 C0 제어문자(U+0001 등)가 있으면
XML 1.0이 그 문자를 문서에 담는 것 자체를 금지하므로 (`&#1;`로도 못 쓴다)
`xmllint`가 문서를 거부했다. `escape_markup`이 XML/HTML 양쪽에서 U+FFFD로
치환하도록 고쳤고, 덤으로 단독 CR은 `&#13;`으로 써서 XML 줄바꿈 정규화와
HTML5 입력 전처리에 먹히지 않게 했다.

(macOS 기본 `tidy`는 2006년판이라 HTML5도 UTF-8도 못 읽어 `<!DOCTYPE html>`과
한글을 전부 오류로 보고한다 — 그래서 쓰지 않는다.)

**GUI 경로 검증 — 실제 앱을 돌려서 확인.** `src/bin/verify_result_export_ui.rs`가
진짜 `MainWindow`를 콜백까지 붙여서 띄우고, **앱 자신의 메뉴 바**로 내보내기를
시작한 뒤, 열린 **프로덕션 모달**을 그 모달의 이벤트 루프 안에서 타임아웃으로
조작한다(포맷 Choice·범위 라디오·대상 라디오 설정 후 Export 클릭). 스텁은 없고
마우스만 대체된다. 클립보드 대상은 `pbpaste`로 전 포맷을 바이트 단위 비교하고,
선택 영역 범위·`SQL Inserts` 숨김·Cancel 무동작까지 확인한다.

이 검증이 **두 번째 실제 버그를 잡았다**: CSV/TSV가 **클립보드에도 UTF-8 BOM을
붙이고 있었다.** BOM은 Excel이 *파일*의 인코딩을 판정하라고 넣는 것이고,
클립보드에 붙으면 붙여넣는 곳마다 보이지 않는 `U+FEFF`가 들어간다.
`render`는 이제 BOM을 만들지 않고, `ExportFormat::file_byte_order_mark()`가
**파일 대상일 때만** 앞에 붙인다. 양쪽 다 확인됨 — `verify_grid_copy_csv`는
여전히 `first 3 bytes = [EF, BB, BF]`, 클립보드는 깨끗하다.

**남은 것**: Excel(xlsx)은 새 크레이트가 필요해 이번 범위에서 제외했다.
파일 대상은 macOS 네이티브 저장 패널에서 멈춘다 — 프로세스 안에서 조작할 수단이
없다. 그 앞단은 클립보드 경로와 같은 코드이고, 쓰기 자체는 `fs::write` 한 줄이다.

### 3. 파일 → 테이블 데이터 임포트 (CSV Import)

- **DataGrip**: 테이블 우클릭 → Import Data from File. 컬럼 매핑, 타입 추론,
  헤더 인식, 배치 커밋.
- **현재 상태**: 임포트 기능 자체가 없다. 소스 전역에 임포트 진입점 없음.
- **왜 시급한가**: 내보내기(CSV)는 되는데 되돌릴 방법이 없다. 개발자가 테스트
  데이터를 넣는 가장 흔한 경로가 막혀 있어서, 사용자가 결국 다른 툴을 켠다.
  "이 앱 하나로 끝난다"는 제품 전제를 직접 훼손하는 구멍.
- **난이도**: 중~대. 다이얼로그(파일 선택 → 구분자/인코딩/헤더 → 컬럼 매핑
  미리보기 → 실행)와 배치 INSERT 실행이 필요. 다만 **staged edit 저장 경로가
  이미 안전한 배치 DML + 롤백을 구현**하고 있으므로 실행 계층은 재사용 가능.
  1차는 "기존 테이블에 CSV append, 헤더=컬럼명 매칭"만으로 충분히 유용하다.

#### 구현 상태 (2026-08-08) — 완료

객체 브라우저에서 테이블 우클릭 → **Import Data...** 하나로 들어간다.
**내보내기가 쓰는 7개 포맷을 전부 읽는다** — CSV/TSV/JSON/XML/HTML/Markdown/
SQL Inserts. 즉 내보낸 파일은 그대로 되돌릴 수 있고, 한 DB에서 뽑은 결과를 다른
DB에 그대로 넣을 수 있다.

| 조각 | 위치 |
| --- | --- |
| 파서 (순수 함수, 테스트 71개) | `src/ui/result_import.rs` |
| INSERT 스크립트 생성기 (순수 함수, 테스트 23개) | `src/ui/table_import.rs` |
| 모달 (포맷 · 헤더 · NULL 텍스트 · 컬럼 매핑) | `src/ui/table_import_dialog.rs` |
| 진입점 | `ObjectBrowserWidget::build_import_script_from_dialog` → `SqlAction::ExecuteScript` |

설계상 결정 네 가지:

- **NULL 규칙이 포맷별로 다르고, 그게 내보내기 규칙의 정확한 역이다.** CSV/TSV는
  설정한 NULL 텍스트와 정확히 일치하는 셀, JSON은 `null` 리터럴, XML은 `<C/>`
  (`<C></C>`는 빈 문자열), HTML/Markdown은 빈 셀, SQL은 `NULL` 키워드.
  그래서 내보내기 → 가져오기 왕복이 값 그대로 돌아온다.
- **리터럴은 대상 컬럼의 선언된 타입이 정한다.** 값의 생김새가 아니라 카탈로그가
  보고한 타입을 `SqlValueKind`로 바꿔 `grid_sql_export::sql_literal_for_value`에
  넘긴다 — `SQL Inserts` 내보내기와 같은 규칙이다. 0으로 채운 코드는 따옴표와
  0을 지키고, `VARCHAR2`에 들어가는 날짜꼴 문자열은 문자열로 남는다.
  빈 문자열이 NULL로 둔갑하지 않도록 NULL 판정이 없는 진입점을 새로 갈랐다.
- **컬럼 매핑은 이름으로, 헤더가 없으면 위치로.** 파일 컬럼마다 선택기가 하나씩
  있고 `(skip)`으로 뺄 수 있다. 옵션이 바뀔 때마다 파일을 다시 읽으므로 화면의
  컬럼 목록·행 수·매핑은 항상 Import가 실제로 실행할 내용이다.
- **배치 INSERT를 평범한 스크립트로 실행한다.** 100행씩 묶어 MySQL/MariaDB는
  다중 행 `VALUES`, Oracle은 `INSERT ALL`로 만든 뒤 F5와 같은 경로로 돌린다.
  커밋 시점은 세션의 auto-commit 설정 그대로고, 실패도 다른 문장과 똑같이 보고된다.

**적대적 유닛 테스트.** 인용부호 안의 구분자·따옴표 안 따옴표·단독 CR·필드 중간
따옴표·닫힌 따옴표 뒤 텍스트·닫히지 않은 따옴표, 중첩 엘리먼트 사이에 끼인 텍스트·
`>`를 담은 속성·CDATA·네임스페이스 접두사, 닫지 않은 `<td>`·대문자 태그·중첩
테이블, 데이터인 `---` 행·이스케이프된 `|`, 문자열 안의 `)`/`,`/`;`/`--`/`/* */`·
따옴표 안의 괄호를 담은 스키마명·여러 줄 INSERT·부호 있는 지수 표기까지 고정했다.
1000행 왕복과 내보내기 테스트가 쓰는 적대적 컬럼명 세트도 포함한다.

**라이브 검증 — 4개 백엔드 × 7개 포맷.** `src/bin/verify_import_live.rs`가
Oracle Thin / Oracle OCI / MySQL / MariaDB 각각에서
`SELECT → 파일로 내보내기 → 디스크에서 다시 읽기 → INSERT 스크립트 → 실행 →
다시 SELECT → 원본과 비교`를 포맷마다 돌린다. 파일은 BOM까지 포함해 실제로
디스크에 쓰고 읽는다. 배치 크기를 2로 줘서 다중 문장 경로도 지난다. 카탈로그가
보고한 선언 타입과 드라이버가 보고한 타입이 **같은 리터럴을 만드는지**도 값마다
대조한다. 전부 통과.

이 검증이 **실제 버그 두 개를 잡았다**:

1. **Oracle `&` 치환.** 값에 `&`가 있으면 클라이언트 측 `DEFINE`이 문자열 리터럴
   *안에서도* 치환을 걸어 "Enter value for T:" 프롬프트에서 멈춘다(`AT&T`).
   파일에서 온 값은 변수일 수 없으므로 `table_import::defuse_substitution`이
   `&`를 `CHR(38)`로 들어낸다. 세션의 `DEFINE` 설정은 건드리지 않는다.
2. **음수 타임존 오프셋이 부호를 잃었다 (기존 결함).**
   `grid_sql_export::oracle_temporal_literal`이 만든
   `TO_TIMESTAMP_TZ('… .000001-05:30','… .FF TZH:TZM')`은 값에 공백이 없는데
   포맷 모델에는 있어서, Oracle이 `TZH`를 부호 자리에서 읽어 `-05:30`을
   `+05:30`으로 저장했다. 기존 유닛 테스트가 이 잘못된 출력을 고정하고 있었고
   라이브 게이트의 픽스처에는 음수 오프셋이 없어 살아남아 있었다. 이제 오프셋을
   공백으로 붙여 쓴다 — **`SQL Inserts` 내보내기도 같이 고쳐졌다.**

**GUI 경로 검증.** `src/bin/verify_import_ui.rs`가 프로덕션 모달을 그대로 띄우고
모달 자신의 이벤트 루프 안에서 조작한다: 포맷 전환 시 파일 재파싱과 매핑 재구성,
포맷별 헤더/NULL 선택지 비활성화, 헤더 없는 파일의 위치 매핑, `(skip)` 처리,
NULL 텍스트 반영, Cancel 무동작, 읽을 수 없는 파일에서 파서 사유를 띄우고
아무것도 만들지 않는 것까지 확인한다.

부수적으로 Oracle 테이블 컨텍스트 메뉴의 **`Generate DDL` 라벨이 깨져 있던 것도
고쳤다** — 줄 연결 백슬래시가 빠져 라벨이 `Generate` + 공백 18개 + `DDL`이었고,
핸들러의 `"Generate DDL"`과 맞지 않아 Oracle 테이블에서는 이 메뉴가 아무 일도
하지 않았다.

**남은 것**: UTF-8이 아닌 파일은 변환하지 않고 이름을 대며 거절한다(CP949 등
인코딩 변환은 범위 밖). Markdown은 셀을 trim하고 줄바꿈을 `<br>`로 쓰므로 앞뒤
공백과 단독 CR은 왕복되지 않고, HTML/Markdown은 NULL과 빈 문자열을 같게 쓴다.
`INSERT ... SELECT` 형태나 컬럼 목록이 없는 INSERT는 매핑할 컬럼이 없으므로
사유를 밝히고 거절한다.

### 4. 값 뷰어/에디터 패널 (LOB·긴 텍스트·JSON)

- **DataGrip**: 그리드 옆 Value 패널에서 셀 값을 크게 보고, **편집**하고,
  JSON/XML은 포맷팅해서 본다. 단일 행을 세로로 보는 Transpose(단일 레코드 뷰)도
  같은 맥락.
- **현재 상태**: `show_cell_text_dialog`(`src/ui/result_table.rs:1058`)가 모달
  창에 `TextDisplay`로 **읽기 전용** 표시만 한다. 포맷팅·검색·편집 없음.
  Transpose 뷰 없음.
- **왜 시급한가**: CLOB/JSON 컬럼을 다루는 순간 바로 막힌다. 특히 **긴 값을 편집할
  방법이 없어서** staged edit이 짧은 값 전용 기능처럼 되어 있다(그리드 편집의
  4000바이트 리터럴 상한 이슈와도 맞물린다).
- **난이도**: 소~중. 기존 모달을 `TextEditor`로 바꾸고 staged edit에 값을
  되돌려주는 것부터. JSON 포맷팅과 Transpose 뷰는 후속.

#### 구현 상태 (2026-08-08) — 완료 (뷰어 · 편집 · JSON/XML 포맷)

셀 더블클릭 또는 그리드 컨텍스트 메뉴 **View Value...** / **Edit Value...** 가
값 창을 연다.

| 조각 | 위치 |
| --- | --- |
| 포맷터·탐지·크기 (순수 함수, 테스트 14개) | `src/ui/value_viewer.rs` |
| 창 | `value_viewer::show` |
| 진입점 | `ResultTableWidget::open_cell_value_window` |
| 값 반영 (인라인 에디터와 공유) | `ResultTableWidget::apply_cell_edit_value` |
| GUI 검증 | `src/bin/verify_value_viewer_ui.rs` |
| 라이브 검증 | `src/bin/verify_value_edit_live.rs` (4 백엔드) |
| 캡쳐 | `capture_feature_tour value-viewer` |

설계상 결정 네 가지:

- **읽기 전용은 `TextDisplay`, 편집 가능은 `TextEditor`.** FLTK `TextEditor`에는
  읽기 전용 플래그가 없다. 키 바인딩을 걷어내 흉내 내는 대신, **이미 표시 전용인
  위젯**을 쓴다 — 선택·스크롤·복사는 그대로 된다.
- **Format은 편집이 아니라 보기다.** 정렬본은 체크가 켜져 있는 동안만 버퍼에
  살고, 편집 중이던 원본은 따로 보관됐다가 저장된다. CLOB을 읽으려고 Format을
  눌렀다가 저장해서 **DB의 공백이 바뀌는 일이 없다.** GUI 검증이 이걸 실제
  위젯으로 고정한다.
- **포맷터는 공백만 옮긴다.** JSON은 문서 모델로 왕복하지 않는다
  (`serde_json::Value`를 거치면 키 순서가 바뀌고 큰 수가 `f64`를 통과한다).
  검증된 토큰 목록을 다시 배치할 뿐이라 키 순서와 숫자 표기가 그대로다. XML은
  **자식이 요소뿐인 요소만** 들여쓴다 — 텍스트가 섞인 요소의 공백은 내용이다.
- **편집 가능 여부를 창이 닫힌 뒤 다시 묻는다.** 창이 열려 있는 동안 저장이
  시작될 수 있다.

**Oracle 긴 값 저장을 같이 고쳤다.** 이게 이 항목이 지목한 실제 통증이다.

- 4000바이트를 넘는 문자열 리터럴은 `ORA-01704`다 →
  `oracle_text_literal`이 `TO_CLOB('..') || TO_CLOB('..')`로 쪼갠다. 청크는
  **이스케이프 전 값**에서 자르므로 경계가 `''` 쌍 가운데나 문자 중간에 떨어질
  수 없다. 한계 아래에서는 바이트 단위로 예전과 같다.
- **길이와 무관한 벽이 하나 더 있었다.** `clob_column = 'text'`는 길이에 관계없이
  `ORA-22848`이라, **CLOB 컬럼이 있는 테이블은 애초에 편집이 안 됐다.**
  라이브 검증이 이걸 잡았다. `original_value_predicate`가 문자 LOB에는
  `DBMS_LOB.COMPARE(col, ..) = 0`을, 나머지에는 종전의 `col = ..`을 낸다.
  LOB 판정은 드라이버가 보고한 **선언 타입**(`column_data_types`)으로 하며 값
  길이로 추측하지 않는다. 가드를 버리지 않고 유지한다는 점이 중요하다.
- MySQL/MariaDB는 바인드라 원래 문제가 없었다. 라이브 검증은 그래도 4개 백엔드
  모두에서 11,200바이트 → 15,200바이트 → 짧은 값 왕복을 바이트 단위로 비교한다
  (한글·작은따옴표·개행 포함).

#### 사후 정밀 리뷰 (2026-08-08) — 결함 4건 수정

커밋 후 정밀 리뷰에서 **테스트가 통과하는데도 남아 있던** 결함 넷을 찾아 고쳤다.
넷 다 "내가 짠 테스트가 내가 짠 코드와 같은 가정을 공유해서" 못 잡은 것들이다.

- **JSON 검증기가 `{"a"}`를 유효로 받았다.** 객체 멤버가 `key : value` 꼴이어야
  한다는 규칙이 없었다. 국소 조건문 뭉치라 읽어서는 알 수 없었던 게 원인이라,
  명시적 상태 기계로 갈아엎었다(`JsonExpect`). 상태가 곧 규칙이 되므로 뒤따르는
  트레일링 콤마(`{"a":1,}`)·`{1:2}`·`[1:2]`도 함께 막힌다. 데이터를 망치는 버그는
  아니었지만(Format은 보기 전용) 유효하지 않은 문서를 "JSON"이라 주장했다.
- **직전 결과의 컬럼 타입이 다음 결과로 새어들 수 있었다.** `start_streaming`이
  `column_data_types`를 비우지 않아, 컬럼 수만 우연히 맞으면 **LOB이 아닌 컬럼을
  LOB으로 표시**해 서버가 거부할 술어를 낼 수 있었다. 헤더를 갈아끼우는 그 자리에서
  같이 비운다.
- **객체 브라우저의 읽기 전용 플래그가 갱신되지 않았다.** 생성 시 한 번만 읽어서,
  커넥션을 읽기 전용으로 바꾸고 재접속해도 그 세션 내내 Drop/Truncate를 계속
  띄웠다(실행 게이트가 막으므로 안전 구멍은 아니지만 "실패할 항목을 띄우지 않는다"
  원칙 위반). `refresh_runtime_labels`에서 런타임 기준으로 재동기화한다.
- **선택 영역 안을 우클릭하면 엉뚱한 셀이 열렸다.** 대상이 선택의 좌상단이라
  포인터 아래 셀과 달랐다. 클릭한 셀을 우선한다.

부수적으로 `detect_value_format`이 버튼 활성화 여부만 정하면서 **정렬본 전체를
만들었다가 버리고 있었다** — 큰 CLOB에서 포맷팅 비용을 두 번 낸다. 검증만 하도록
바꿨다. 2.4 MB JSON 기준 디버그 빌드에서 탐지 65 ms / 포맷 115 ms, 평문은 0 ms
(첫 글자에서 끝난다). 선형 복잡도를 테스트로 고정했다.

**검증 공백도 하나 메웠다.** 유닛 테스트가 `character_lob_columns`를 손으로 채운
세션에 대고만 돌아서, **드라이버가 실제로 LOB 타입을 보고하는지**는 아무도 확인하지
않고 있었다. `verify_value_edit_live`에 그 확인을 넣었다 — thin은 `"Clob"`,
OCI는 `"CLOB"`으로 보고한다. 대소문자가 다르므로 정규화가 실제로 필요했다.

**남은 것**: Transpose(단일 레코드) 뷰와 값 창 내부 검색. 원래 계획대로 후속이다.
BLOB 편집은 여전히 범위 밖이다 — 그리드가 보여주는 값이 자리표시자라 어떤 비교도
옳을 수 없다.

### 5. 객체 브라우저에서의 파괴적/구조 변경 작업 (Drop / Truncate / Rename)

- **DataGrip**: 객체 우클릭 → Drop, Truncate, Rename(리팩터링), 그리고
  Modify Table 다이얼로그로 컬럼/인덱스/제약 편집.
- **현재 상태**: 객체 컨텍스트 메뉴는 **전부 읽기 전용**이다 —
  `Select Data (Top 100) | View Structure | View Indexes | View Constraints |
  Generate DDL | Execute Procedure/Function | Check Compilation`
  (`src/ui/object_browser.rs:6974`, `7558`). Drop/Create/Alter/Rename/Truncate
  라벨이 소스에 존재하지 않는다.
- **왜 시급한가**: 스키마를 만지는 흐름이 통째로 SQL 수기 작성으로 떨어진다.
  단 **Modify Table 같은 풀 DDL 에디터는 Tier 0이 아니다.** 1차 범위는
  `Drop` / `Truncate`(확인 다이얼로그 + 생성될 DDL 미리보기 + 실행)까지로
  좁혀야 한다. 이 앱은 "몰래 아무것도 하지 않는다"를 내세우므로,
  **미리보기 후 실행** 형태가 제품 성격에도 맞는다.
- **난이도**: 소(Drop/Truncate) / 대(Modify Table 다이얼로그).

#### 구현 상태 (2026-08-07) — 완료 (Drop / Truncate)

객체 컨텍스트 메뉴 끝에 `Truncate...`(테이블)와 `Drop...`(백엔드가 이름만으로
DROP할 수 있는 타입)이 붙었다 — `DestructiveObjectAction`
(`src/ui/object_browser.rs:100`), 메뉴 문자열(`object_browser.rs:7181`, `7799`).

- **클릭으로 실행되지 않는다.** 실행될 문장을 그대로 보여주는 확인 다이얼로그를
  거친 뒤, 그 문장을 에디터에 넣고 일반 실행 경로로 실행한다. 끝나면 사용자
  손에 방금 실행된 SQL이 남는다. 계획대로 "미리보기 후 실행"이다.
- **문장은 읽은 그대로다.** `CASCADE CONSTRAINTS`도 `PURGE`도 붙이지 않는다.
  DB가 거부하면 더 넓은 문장으로 조용히 재시도하지 않고 그 에러를 그대로 보고한다.
- **인덱스는 일부러 뺐다.** `DROP INDEX`는 소속 테이블이 필요한데 트리 노드가
  그 정보를 들고 있지 않다.
- DDL 문자열은 백엔드마다 한 곳에서만 만들고, 그 함수가 문장을 돌려줄 때만 메뉴
  항목이 뜬다. 전 백엔드 × 전 객체 타입을 도는 테스트가 둘의 일치를 고정하므로,
  "눌러도 반드시 실패하는 항목"이 뜰 수 없다.

**남은 것**: `Rename`과 Modify Table(풀 DDL 에디터). 원래 계획대로 1차 범위 밖이다.

---

## Tier 1 — 중요. 없으면 계속 불편하지만 우회는 가능

### 6. 그리드 컬럼 숨김 / 순서 변경 / 고정(pin)

- **현재 상태**: 없음(`result_table.rs`에 reorder/hide 관련 코드 없음). 컬럼 폭
  조절과 정렬만 된다.
- **영향**: 컬럼 40개짜리 테이블을 볼 때 키 컬럼이 화면 밖으로 밀린다.
- **난이도**: 중(표시 인덱스 ↔ 데이터 인덱스 매핑 도입 필요. 숨김 컬럼이
  CSV/SQL 내보내기·staged edit의 컬럼 인덱스와 충돌하지 않도록 주의).

### 7. 선택 영역 집계 표시 (건수/합/평균/최소/최대)

- **현재 상태**: 없음.
- **영향**: 숫자 컬럼 드래그해서 합계 확인하는 동작은 DataGrip 사용자가
  무의식적으로 쓴다. 없으면 매번 별도 집계 쿼리를 쓴다.
- **난이도**: 소. 이미 선택 범위 모델이 있으므로 상태 바 텍스트만 추가.

#### 구현 상태 (2026-08-08) — 완료

`src/ui/selection_summary.rs`(순수 함수, 테스트 22개). 두 셀 이상을 고르면
상태 바 오른쪽에 `Count / Sum / Avg / Min / Max`가 붙고, 한 셀로 줄면 사라진다.
서버로 아무것도 보내지 않는다 — 그리드가 이미 들고 있는 값만 본다.

| 조각 | 위치 |
| --- | --- |
| 집계 (파싱·정확 십진 누산·라벨) | `src/ui/selection_summary.rs` |
| 선택 범위 → 집계 (메모이즈) | `ResultTableWidget::selection_summary_label` |
| 화면에 보이는 그리드만 대상 | `ResultTabs::selection_summary_label` |
| 상태 바 렌더 | `StatusBarWidget::set_selection_summary` |
| 캡처 검증 | `capture_feature_tour selection-summary` |

설계상 결정 세 가지:

- **SQL 집계 의미론을 따른다.** NULL은 건너뛰고 `Count`는 선택한 *셀* 수가
  아니라 **NULL이 아닌 값**의 수다. 1번 항목에서 로컬 평가기를 거부한 것과 같은
  이유로, 여기서도 자체 규칙을 만들지 않고 SQL이 이미 정한 규칙을 쓴다.
- **합은 f64가 아니라 정확한 십진 누산이다.** 값은 전부 문자열이므로 드라이버가
  낸 자릿수를 그대로 `i128` 스케일 정수로 받아 더한다. `0.1 + 0.2`는 `0.3`이고
  38자리 Oracle `NUMBER`도 자릿수를 잃지 않는다. `i128` 범위를 넘으면 근사값을
  내놓는 대신 숫자 집계를 통째로 버리고 `Count`만 남긴다(테스트로 고정).
- **숫자가 아닌 값이 하나라도 있으면 `Count`만 낸다.** 천 단위 구분자·날짜·문자열
  섞인 선택에서 합계를 지어내지 않는다. 200만 셀을 넘는 선택은 스캔하지 않고
  선택 크기만 보고한다(상태 바가 애니메이션 프레임마다 물어보기 때문에,
  집계는 선택·행 세대·행 수가 바뀔 때만 다시 돈다).

### 8. 코드 스니펫 / 라이브 템플릿

- **현재 상태**: 없음(IntelliSense는 메타데이터·키워드 보완만).
- **영향**: `sel<Tab>` → `SELECT * FROM ... WHERE`, `ins<Tab>` 같은 반복 타이핑
  단축이 없다.
- **난이도**: 중. 완성 파이프라인(`merge_completion_sources`)에 스니펫 소스를
  하나 더 붙이는 구조로 갈 수 있으나, 플레이스홀더 순회(Tab 이동) UI가 새 작업.

#### 구현 상태 (2026-08-08) — 완료

`src/ui/sql_editor/snippets.rs`(순수 함수 + 에디터 세션, 테스트 14개).
`sel` + `Tab` → `SELECT * / FROM ${table} / WHERE ${condition}`, 첫 플레이스홀더가
선택된 상태로 들어가고 `Tab`이 다음으로, `Esc`가 템플릿을 빠져나온다.
내장 템플릿 12개(`sel selc ins upd del join ljoin case ct beg ife forl`).

| 조각 | 위치 |
| --- | --- |
| 템플릿 표·본문 파싱·재탐색 | `src/ui/sql_editor/snippets.rs` |
| 에디터 세션 (확장/이동/취소) | 같은 파일의 `impl SqlEditorWidget` |
| 키 처리 (`Tab`, `Ctrl+J`, `Esc`) | `intellisense/runtime.rs` KeyDown |
| 목록 다이얼로그 | `menu::show_snippet_reference_dialog` (Help > Code Snippets) |
| 캡처 검증 | `capture_feature_tour code-snippets` |

설계상 결정 세 가지:

- **완성 파이프라인에는 손대지 않았다.** 스니펫을 `merge_completion_sources`의
  소스로 넣으면 후보 목록이 바뀌어, 55회 이상의 정밀도 감사로 수렴시킨 244개
  precision 테스트와 감사 픽스처를 전부 흔든다. 대신 **트리거 키**로 갔다:
  팝업이 떠 있으면 `Tab`은 지금까지처럼 선택된 후보를 넣고, 스니펫은 `Ctrl+J`가
  맡는다. 팝업이 없을 때만 `Tab`이 약어를 펼친다.
- **플레이스홀더 위치를 저장하지 않는다.** 사용자가 앞 플레이스홀더에 긴 이름을
  치면 뒤 위치는 전부 밀린다. 그래서 세션은 위치 대신 **구분 리터럴**(`\nWHERE `
  같은)만 들고 있다가 `Tab` 때 커서 앞에서 그것을 찾아 다시 앵커한다. 리터럴이
  사라졌으면(사용자가 템플릿을 뜯어고쳤으면) 추측하지 않고 세션을 끝내고
  `Tab`은 원래 하던 일로 돌아간다. 되돌리기 방향(`Shift+Tab`)은 같은 방식으로
  안전하게 만들 수 없어 넣지 않았다 — 전진만 한다.
- **`Tab`은 세션 중에도 텍스트를 절대 바꾸지 않는다.** 커서와 선택만 움직이므로,
  재앵커가 엉뚱한 곳을 짚어도 최악이 "커서가 이상한 데로 갔다"이지 문장이
  망가지는 일은 없다. 커서가 마지막으로 방문한 플레이스홀더보다 앞에 있으면
  세션을 끝낸다(사용자가 템플릿 밖으로 나간 것).

### 9. 에디터 멀티 커서 / 열(컬럼) 선택, 라인 이동·복제

- **현재 상태**: 없음. `Alt+드래그` 열 선택, `Ctrl+D` 라인 복제, `Alt+Shift+↑/↓`
  라인 이동 모두 미구현(단축키 목록 `src/ui/menu.rs:556-613` 참조).
- **영향**: 컬럼 목록을 손으로 다듬는 작업(50줄에 쉼표 붙이기 등)이 수작업이 된다.
- **난이도**: 대. FLTK `TextEditor` 위에서 멀티 커서는 사실상 커서 모델 재작성.
  **열 선택과 라인 이동/복제만 먼저 하는 편이 비용 대비 효과가 크다.**

### 10. 객체 정의로 이동 (Go to Declaration) / 전역 객체 검색

- **현재 상태**: `Ctrl+Click`은 Quick Describe만 띄운다. 객체 이름으로 소스
  정의(뷰/프로시저 본문)를 여는 동작이나, 트리 밖에서의 전역 검색
  (DataGrip `Ctrl+N` / Search Everywhere)은 없다. 객체 트리 필터 입력만 있다.
- **영향**: 프로시저 이름을 알 때 정의를 여는 최단 경로가 없다.
- **난이도**: 중. Quick Describe 인프라(커서 위치 → 객체 해석)를 재사용해
  "정의 열기" 액션을 추가하는 형태.

#### 구현 상태 (2026-08-08) — 완료

`Ctrl+B`가 커서 아래 객체의 소스를 새 에디터 탭에 열고, `Ctrl+Shift+N`이 이름으로
객체를 찾는다. 둘 다 같은 종착점(`spawn_generate_ddl` → `SqlAction::OpenInNewTab`)을
쓴다.

| 조각 | 위치 |
| --- | --- |
| 이름 랭킹 (순수 함수, 테스트 19개) | `src/ui/object_search.rs` |
| 검색 모달 (테스트 4개) | `src/ui/object_search_dialog.rs` |
| 커서 아래 이름 후보 | `SqlEditorWidget::object_context_candidates_at_cursor` |
| 이름 → 객체 → 소스 | `ObjectBrowserWidget::open_declaration_for_sql_selection` / `declaration_target_for_item` |
| 진입점 | `MainWindow::go_to_declaration_at_cursor` / `open_object_search` |
| 라이브 검증 (4개 백엔드) | `src/bin/verify_explain_plan_live.rs` |
| 캡쳐 검증 | `capture_feature_tour object-search` |

설계상 결정 네 가지:

- **해석기를 새로 만들지 않았다.** 우클릭 컨텍스트 메뉴가 이미 쓰는
  `resolve_selected_object_context`(1·2·3부 이름, 패키지 루틴, 스코프 정규화)를
  그대로 부른다. 그래서 `Ctrl+B`가 여는 것은 **같은 이름에 대해 트리가 여는 것과
  정확히 같다.** 이름 판정 규칙이 두 벌 생기지 않는다.
- **`default_action_for_item`은 일부러 쓰지 않았다.** 트리에서 테이블을 더블클릭하면
  데이터를 보지만, "정의로 이동"은 언제나 정의여야 한다. 그래서 테이블도 DDL을 연다.
  패키지 멤버는 자기 DDL이 없으므로 패키지를 연다 — 트리가 패키지 자식에 대해 하는
  선택과 같다.
- **전역 검색은 이미 캐시된 현재 스코프만 본다.** 서버로 조회하지 않으므로 즉시
  답하고, 트리에 없는 것을 보여줄 수 없다. 트리 필터와 다른 점은 **평면 랭킹 목록 +
  키보드로 바로 열기**다: 정확 일치 → 접두 일치 → 부분 일치 순, 같은 등급이면 짧은
  이름이 위. 패키지 멤버는 맨이름으로도 `PKG.MEMBER`로도 찾히고 표시는 항상 한정된다.
- **에디터 단축키는 메뉴 액션 하나를 부른다.** `Ctrl+B`/`Ctrl+Shift+N`은
  `menu_action_callback`으로 메뉴 경로를 넘기므로 구현이 `execute_menu_action` 한
  곳에만 있다. 메뉴에서 눌렀을 때와 키로 눌렀을 때가 갈릴 수 없다.

**남은 것**: 서버 전역 검색(`ALL_OBJECTS` / `INFORMATION_SCHEMA` 질의)은 범위 밖.
현재 스코프에 없는 객체는 스코프를 바꾼 뒤 찾아야 한다.

### 11. 커넥션 색상 구분 + 읽기 전용(프로덕션 보호) 모드

- **DataGrip**: 데이터 소스마다 색을 지정해 에디터/탭에 표시. 데이터 소스를
  read-only로 잠글 수 있음.
- **현재 상태**: 둘 다 없음(`src/ui/connection_dialog.rs`에 색상 설정 없음,
  읽기 전용 커넥션 플래그 없음).
- **왜 여기 있나**: 기능 편의보다 **사고 방지**다. "prod에 실수로 DELETE"를
  막는 최소 장치이고, 이 앱의 "안전 우선" 서사와 정확히 일치한다. 구현 비용도
  낮아서 가성비가 가장 좋은 항목 중 하나.
- **난이도**: 소(색상) / 소~중(read-only — 실행 전 문 분류로 DML/DDL 차단.
  `src/db/sql_classification.rs`가 이미 문 종류를 판정하므로 재사용 가능).

#### 구현 상태 (2026-08-08) — 완료

커넥션 다이얼로그 **Connection Info** 열에 `Color:`와 `Safety: Read-only` 두 줄.

| 조각 | 위치 |
| --- | --- |
| 저장 | `ConnectionInfo::color` / `read_only` (`src/db/connection.rs`) |
| 색 팔레트 | `ConnectionColor` (`label` / `rgb` / `ALL`) |
| 폼 | `src/ui/connection_dialog.rs` |
| 차단 판정 (순수 함수) | `sql_classification::read_only_block_reason` |
| 관문 | `SqlEditorWidget::read_only_refusal` (에디터 단일 깔때기) |
| 메뉴 필터 | `ObjectBrowserWidget::menu_choices_for_read_only` |
| 라이브 검증 | `src/bin/verify_read_only_live.rs` (4 백엔드) |
| 캡쳐 | `capture_feature_tour connection-color` |

설계상 결정 다섯 가지:

- **둘 다 Advanced Settings가 아니라 Connection Info에 있다.** 세션 옵션이 아니고
  서버로 나가지도 않는다. 저장된 프로필의 성질이므로 DB 종류를 바꿔도 살아남는다.
- **`ConnectionInfoSerde`에도 반드시 넣어야 한다.** `ConnectionInfo`의
  `Deserialize`는 수동 구현이라 그 미러에 빠진 필드는 **저장은 되는데 다시 읽을 때
  조용히 사라진다.** 왕복 테스트로 고정했다.
- **끊긴 상태가 색상보다 우선한다.** 태그 색은 "connected" 초록만 대체하고
  disconnected 회색은 절대 덮지 않는다. 살아 있는 세션인지가 표시등이 언제나 말할
  수 있어야 하는 유일한 것이다.
- **문장 단위로 판정한다.** 통짜 텍스트를 분류하면 SELECT 세 개짜리 F5 스크립트가
  `Script`로 뭉뚱그려져 막힌다 — 읽기 전용 커넥션이야말로 여러 쿼리를 한꺼번에
  돌리는 게 정상인 곳이다. 실행기가 쓸 것과 **같은** 스플리터·같은 MySQL
  delimiter로 쪼갠 뒤 각각을 본다. 분류 못 한 문장은 통과시키지 않는다.
  `@file` 인클루드(내용을 미리 볼 수 없다)와 `CONNECT`(커넥션 자체를 빠져나간다)도
  막는다.
- **막을 것은 애초에 띄우지 않는다.** 그리드 **Edit** 체크박스가 사라지고, 객체
  브라우저에서 Drop/Truncate/Import Data/Execute Procedure·Function이 빠진다.
  카탈로그를 읽는 Generate DDL·View Structure·Check Compilation은 그대로다.
  Drop/Truncate 때 세운 "눌러도 반드시 실패하는 항목이 뜰 수 없다"와 같은 원칙.

관문은 `execute_sql_with_mysql_delimiter_after_lazy_cancel` — 소스 주석이 이미
"All editor execution entry points funnel through this SQL-aware preflight"라고
못박은 자리다. **트랜잭션 preflight와 바인드 프롬프트보다 앞**에 둔다: 거절할
문장의 플레이스홀더 값을 먼저 물어보는 일이 없어야 한다.

**서버측 강제가 아니다.** 실수를 막는 장치이고, 작정한 시도를 막지는 못한다.
README와 `docs/session.md`에 그렇게 적었다. 서버가 강제하길 원하면 커넥션의
`Access: Read only` 트랜잭션 모드나 쓰기 권한 없는 계정을 쓰면 된다 — 둘 다 이미
있다.

**라이브 검증에는 대조군이 있다.** 거절 대상 문장을 **먼저 쓰기 가능 커넥션에서
돌려 성공을 확인**한 뒤에야 읽기 전용 커넥션에서 거절을 확인한다. 대조군이 없으면
백엔드가 어차피 거부했을 문장과 가드가 막은 문장이 구분되지 않는다 — 오타 하나가
통과로 읽힌다. 이어서 **두 번째 쓰기 가능 커넥션**을 증인으로 세워 거절 뒤 행 수와
스키마(생성/이름변경/컬럼추가/삭제)가 그대로인지 확인하고, 마지막으로 같은
프로세스의 쓰기 가능 커넥션은 여전히 쓸 수 있는지 확인한다(가드가 전역 모드가
아니라 **커넥션의 성질**임을 고정).

백엔드별 방언까지 돈다 — Oracle은 MERGE·INSERT ALL·PL/SQL·DECLARE 블록·
CREATE OR REPLACE PROCEDURE·GRANT·TRUNCATE·ALTER, MySQL 계열은 REPLACE INTO·
ON DUPLICATE KEY UPDATE·실행 주석(`/*! ... */`) 속 DELETE·RENAME TABLE·
CREATE PROCEDURE. 읽기 쪽은 SELECT·WITH·다중 SELECT·스칼라 서브쿼리+조인·
COMMIT·ROLLBACK, Oracle은 ALTER SESSION SET CURRENT_SCHEMA, MySQL 계열은
SHOW TABLES·DESCRIBE·USE·EXPLAIN.

**읽기처럼 보이지만 거절되는 두 가지를 라이브 검증이 잡아냈다.**
`SELECT ... FOR UPDATE`는 문장보다 오래 사는 행 잠금을 잡고,
Oracle `EXPLAIN PLAN FOR`는 `PLAN_TABLE`에 행을 **삽입한다**. 분류기가 둘 다
이미 `Dml`로 보고 있었고 그게 옳다 — 대신 **읽기 전용 Oracle 커넥션에서는 F6
실행 계획을 쓸 수 없다.** MySQL 계열 `EXPLAIN`은 보고만 하므로 그대로 된다.
README와 `docs/session.md`에 적었다.

**부수적으로 캡쳐 스크립트 버그 하나를 고쳤다.** `scripts/capture_feature_tour.sh`에
`connection-dialog`·`settings-dialog` 단일 씬 분기가 없어서, 그 모드로 돌리면 맨
아래 전체 변환 목록으로 흘러 **`/tmp`에 남아 있던 오래된 PPM으로 무관한 이미지
8개를 덮어썼다**(main-window와 result-grid가 같은 파일이 되는 식). 문서대로 캡쳐를
돌리다 실제로 밟았다. 분기를 추가하고, 새 씬을 넣을 때 분기도 같이 넣어야 한다는
주석을 달았다.

**남은 것**: 색을 더 많은 곳(결과 탭, 객체 브라우저 트리)에 칠하는 것. 지금은
상태바 표시등·상태바 텍스트·쿼리 탭 라벨 세 곳이다.

### 11-b. 그리드 내 텍스트 검색 (`Ctrl+F`)

- **현재 상태**: 없음. 1번이 서버 재조회로 확정되면서 여기로 분리됐다.
- **1번과의 관계**: 대체재가 아니라 보완재다. 1번은 "조건에 맞는 행만 남긴다"
  (재실행 수반), 이건 "화면에 있는 값을 찾아 하이라이트한다"(재실행 없음).
  DataGrip도 이 둘을 별개로 둔다.
- **난이도**: 소. 이미 fetch된 `full_data`에 대한 단순 부분문자열 검색 +
  일치 셀 강조 + 다음/이전 이동. 평가기가 필요 없으므로 1번의 게이트가 전혀
  적용되지 않는다. **`src/utils/text_search.rs`가 이미 공용 검색 알고리즘을
  갖고 있으니 재사용할 것.**

#### 구현 상태 (2026-08-07) — 완료

`src/ui/grid_search.rs`. 그리드에 포커스가 있을 때 `Ctrl+F`가 **이미 fetch된
행만** 검색한다. 서버로 아무것도 보내지 않고 문장을 다시 돌리지도 않는다 —
1번(필터)과 갈라지는 지점이 정확히 그것이고, 그래서 **필터 바가 재조회할 수 없는
결과에서도 쓸 수 있다.**

- 부분문자열 검색은 에디터 Find가 쓰는 `find_replace::find_next_match` 그대로다.
  에디터에서 걸리는 검색어는 그리드에서도 걸린다(같은 대소문자 접기, 같은 UTF-8
  경계 처리).
- 좁은 셀이 잘라 그린 텍스트가 아니라 **저장된 값**을 찾고, 편집 모드가 만드는
  폭 0의 ROWID 컬럼은 건너뛴다(거기서 맞으면 화면에 넣을 수 없는 셀로 선택이
  이동한다).
- 모든 일치를 옅게, 현재 일치를 같은 색의 진한 톤으로 칠한다. Enter/Next는 앞으로,
  Shift+Enter/Previous는 뒤로 순환한다. 새 검색어는 1행이 아니라 선택된 셀에서
  출발하고, 화면에 이미 있는 일치는 뷰포트를 움직이지 않는다. 하이라이트는
  다이얼로그가 어떻게 닫히든 지워진다.
- 검색 중에 행이 통째로 교체될 수 있다(검색을 열기 전에 시작된 문장이 그 사이
  끝나는 경우). 그리드가 "행이 덧붙여진 게 아니라 교체된 횟수"를 세고 그 값이
  변하면 재스캔하므로, 결과가 도착하기 전에 모아둔 좌표를 밟는 일이 없다.

### 12. 실행 계획 시각화

- **현재 상태**: `F6` Explain Plan 결과를 **일반 결과 그리드**로 표시한다.
- **영향**: 계획 트리의 부모/자식 관계와 비용 비중을 눈으로 못 읽는다.
- **난이도**: 중(트리 위젯 + 비용 비율 강조). 데이터는 이미 있으므로 표현 문제.

#### 구현 상태 (2026-08-08) — 완료

`F6` 결과가 한 컬럼짜리 텍스트가 아니라 **연결선이 그려진 계획 그리드**가 됐다.

| 조각 | 위치 |
| --- | --- |
| 계획 모델·연결선·비용 몫 (순수 함수, 테스트 31개) | `src/ui/explain_plan.rs` |
| Oracle `PLAN_TABLE` 조회 (OCI / thin) | `QueryExecutor::get_explain_plan` / `get_thin_explain_plan` |
| MySQL/MariaDB `EXPLAIN` 원본 결과 | `MysqlExecutor::get_explain_plan` |
| 백엔드 분기 | `trait ExplainPlanBackend` (`sql_editor/mod.rs`) |
| 그리드 조립 | `SqlEditorWidget::build_explain_plan_result` |
| 라이브 검증 | `src/bin/verify_explain_plan_live.rs` |
| 캡쳐 검증 | `capture_feature_tour explain-plan` |

**이 항목은 표현 문제가 아니었다.** 구조 정보가 UI에 닿기 *전에* 버려지고 있었다 —
Oracle은 `PLAN_TABLE`을 아예 읽지 않고 `DBMS_XPLAN.DISPLAY`가 미리 그려 준 ASCII만
받았고, MySQL은 진짜 컬럼과 행을 받아 놓고 `format_explain_lines`가 패딩 문자열로
평탄화했다. 양쪽 다 조회와 반환 타입을 바꿔야 했다.

설계상 결정 네 가지:

- **그리드에 그렸다. FLTK `Tree` 위젯을 넣지 않았다.** `ResultTab.table`이
  non-optional이라 트리를 넣으려면 `result_tabs.rs` 구조를 흔들어야 하고, 그 탭에서
  그리드 검색(`Ctrl+F`)·선택 집계·복사·내보내기를 전부 잃는다. 대신 `Operation`
  컬럼에 연결선을 그렸다 — 계획은 결국 "한 컬럼이 계층인 표"다.
- **연결선은 `PARENT_ID`에서 나온다.** 들여쓰기를 세거나 추측하지 않는다. 형제
  마지막이면 `└─`, 아니면 `├─`, 조상 줄기는 `│`. 라이브 게이트가 **그려진 연결선
  깊이 == 실제 부모 체인 깊이**를 대조한다.
- **`Cost`는 Oracle이 보고한 누적 비용 그대로, `Cost %`는 자기 비용 몫이다.**
  자기 비용 = `cost - Σ자식 cost`(음수는 0). 누적 비용을 그대로 비율로 쓰면 부모가
  자식을 품고 있다는 이유만으로 항상 비싸 보인다. 자기 비용이라야 "어느 단계가
  비싼가"가 읽힌다. 비용이 NULL인 단계가 있으면 몫의 합은 100%가 되지 않는다 —
  없는 값을 지어내지 않기 때문이고, 이는 의도된 결과다.
- **MySQL/MariaDB에는 트리를 지어내지 않는다.** classic `EXPLAIN`에는 부모 컬럼이
  없다. `id`/`select_type`으로 부모를 추정하는 것은 휴리스틱이고, 1번 항목에서 로컬
  평가기를 기각한 것과 같은 이유로 거부했다. 대신 서버가 준 컬럼을 **그대로** 내고
  `rows` 몫만 `Rows %`로 덧붙인다(정확한 산술, 추정 없음).

부수적으로 **Predicate Information 각주가 사라지고 행 자체로 옮겨졌다.**
`ACCESS_PREDICATES`/`FILTER_PREDICATES`가 그 단계의 `Predicates` 칸에 직접 들어가므로
`DBMS_XPLAN`처럼 id를 보고 각주를 찾아갈 필요가 없다.

**라이브 검증 — 4개 백엔드.** `verify_explain_plan_live`가 Oracle Thin / Oracle OCI /
MySQL / MariaDB에서 조인 + 스칼라 서브쿼리 문장을 F6과 같은 경로로 설명시키고,
루트가 하나인지·모든 `parent_id`가 실재하는지·사이클이 없는지·연결선 깊이가 맞는지·
Oracle은 predicate와 cost 몫이 실려 오는지·MySQL 계열은 서버 컬럼이 그대로인지를
확인한다. Thin과 OCI는 바이트 단위로 같은 계획을 낸다.

**남은 것**: `DBMS_XPLAN`의 Note 각주(동적 통계 사용 등)는 `PLAN_TABLE`에 없으므로
표시하지 않는다. MySQL `EXPLAIN FORMAT=JSON` 기반의 진짜 트리는 버전별 구조 차이가
커서 범위 밖으로 뒀다(사용자가 직접 친 `EXPLAIN FORMAT=JSON`은 지금도 그냥 실행된다).

### 13. 외래키 기반 관련 데이터 이동 (Go to Referenced Data)

- **현재 상태**: 없음. 제약 정보 조회(`View Constraints`)는 있으나 데이터 탐색에
  연결되지 않는다.
- **난이도**: 중. FK 메타데이터는 이미 읽고 있으므로, 셀 → FK 대상 테이블
  필터 쿼리 생성으로 구현 가능.


### 24. 결과 정렬 수단 (헤더 정렬 제거 이후)

- **현재 상태**: 2026-08-07에 헤더 정렬이 제거됐다([부록 B](#부록-b-헤더-정렬-제거됨)).
  지금 정렬하는 방법은 **필터 바의 `ORDER BY` 입력뿐**이고, 필터 바는 재조회할 수
  있는 결과에만 붙는다. 따라서 스크립트 산물, 바인드/치환 변수가 든 문의 결과,
  중복 컬럼이 있는 MySQL/MariaDB 결과, 커넥션이 끊긴 결과에는 **정렬 수단이 0이다.**
- **왜 여기 있나**: 정렬은 그리드에서 가장 자주 하는 동작인데, 지금은 되는 결과와
  안 되는 결과가 갈리고 그 경계가 화면에 드러나지 않는다.
- **선택지**:
  - (a) 재조회 가능한 결과에서 헤더 클릭 → 필터 바의 `ORDER BY` 칸을 채워주기만
    한다(실행은 사용자가). 정렬 상태를 그리드가 들지 않으므로 제거된 코드가
    돌아오지 않는다. **가장 작다.**
  - (b) 재조회 불가능한 결과에만 로컬 정렬을 되살린다. `grid_sort.rs`를 커밋
    `f635179`에서 그대로 꺼내면 되지만(테스트 27개 포함), 그리드에 정렬 상태가
    다시 생긴다.
  - (c) 현행 유지 + "정렬은 ORDER BY로"를 UI에 명시.
- **난이도**: 소(a) / 중(b).

### 25. 셀 값으로 빠른 필터 (Filter by / Exclude)

- **DataGrip**: 셀 우클릭 → 그 값으로 필터. 필터 문자열을 손으로 안 친다.
- **현재 상태**: 없음. 그리드 컨텍스트 메뉴는 `Export Results | Copy |
  Copy with Headers | Copy All | SQL Inserts | SQL Updates | Where Clause |
  Set Null` 뿐이다(`src/ui/result_table.rs:7279`).
- **왜 시급한가**: 1번(결과 필터)이 들어온 지금, 이게 그 기능의 실사용 진입점이다.
  현재는 필터를 쓰려면 컬럼명과 리터럴 표기를 사용자가 직접 타이핑해야 한다.
- **난이도**: 소. `Where Clause` 내보내기가 이미 **선택 셀 → 타입에 맞는 SQL
  리터럴 조건문**을 만든다(`src/ui/grid_sql_export.rs`). 그 문자열을 클립보드
  대신 필터 바의 `WHERE` 입력에 넣으면 끝이다. 날짜·NULL·문자열 이스케이프 판정이
  이미 드라이버 타입 기반이라 새 평가기가 생기지 않는다.

### 26. 객체 트리에서 데이터 내보내기 / 가져오기

- **DataGrip**: 테이블 우클릭 → Export Data / Import Data from File. 결과 그리드를
  거치지 않는다.
- **현재 상태**: **가져오기는 들어갔다** — 3번이 완료되면서 테이블 컨텍스트
  메뉴에 `Import Data...`가 생겼다. 남은 건 내보내기 쪽이다. 내보내기는 여전히
  **결과 그리드에만** 있어서(2번), 테이블 하나를 파일로 뽑으려면 먼저
  `Select Data`로 열어야 한다.
- **왜 여기 있나**: 2번과 3번이 끝난 지금 **남는 일이 메뉴 항목 하나**라서다.
  비용 대비 체감이 가장 큰 축에 든다.
- **난이도**: 소. 2번의 모달·직렬화기와 3번의 임포트 엔진을 그대로 부르고 대상
  테이블만 트리에서 얻는다. 다만 큰 테이블은 전체 fetch를 유발하므로 2번의
  `LazyFetchPendingAction::Export`와 같은 진행 표시·취소가 필요하다.

### 27. 테이블 노드 확장 → 컬럼 목록

- **DataGrip**: 트리에서 테이블을 펼치면 컬럼/키/인덱스가 자식 노드로 나오고,
  컬럼을 에디터로 드래그할 수 있다.
- **현재 상태**: 트리는 **카테고리 → 객체 2단계에서 끝난다** (`ObjectItem`이
  `Simple` / `PackageRoutine` 둘뿐, `src/ui/object_browser.rs:375`). 컬럼은
  `View Structure` 모달로만 본다. 드래그 페이로드(`object_drag_payload.rs`)도
  객체 이름 단위다.
- **영향**: 컬럼 이름 하나 확인하려고 모달을 열고 닫는다. IntelliSense가 있어서
  치명적이진 않지만, 트리를 "구조를 보는 곳"으로 쓰는 흐름이 끊긴다.
- **난이도**: 중. 펼칠 때 조회하는 지연 로딩과 캐시가 필요하다. 메타데이터 조회
  경로 자체는 이미 있다(View Structure / IntelliSense가 쓰는 것).

### 28. 에디터 기본 편의 — 소프트 랩 · Go to Line

- **현재 상태**: 에디터는 `WrapMode::None` 고정이라(`src/ui/sql_editor/mod.rs:2798`)
  긴 줄은 가로 스크롤로만 볼 수 있고 토글이 없다. 줄 번호로 이동하는
  `Ctrl+G`도 없다(`go_to_line` 류 코드 없음). 코드 폴딩도 없다.
- **영향**: 생성된 DDL이나 긴 `IN (...)` 목록을 볼 때 가로 스크롤을 계속 민다.
  스크립트 실행 에러는 줄 번호를 알려주는데 그 줄로 가는 최단 경로가 없다.
- **난이도**: 소(랩 토글은 `wrap_mode` 한 줄 + 설정 저장, Go to Line은 작은 모달) /
  대(폴딩 — FLTK `TextEditor`에 폴딩 개념이 없어 사실상 직접 구현이라 Tier 2다).

#### 구현 상태 (2026-08-08) — 완료 (소프트 랩 · Go to Line)

**Edit > Soft Wrap** 토글과 `Ctrl+G` Go to Line.

| 조각 | 위치 |
| --- | --- |
| 설정 | `AppConfig::editor_soft_wrap` (`src/utils/config.rs`) |
| 적용 | `SqlEditorWidget::set_soft_wrap` · `MainWindow::apply_soft_wrap_setting` |
| 줄 번호 파싱 (순수 함수, 테스트 9개) | `SqlEditorWidget::parse_goto_line_input` |
| 이동 | `SqlEditorWidget::prompt_go_to_line` → `go_to_line_index` |
| GUI 검증 | `src/bin/verify_editor_convenience_ui.rs` |
| 캡쳐 검증 | `capture_feature_tour soft-wrap` |

설계상 결정 네 가지:

- **랩 상태는 생성자가 설정에서 읽는다.** 토글이 열려 있는 탭을 순회하는 것과 별개로
  `SqlEditorWidget::new`가 `AppConfig::runtime()`을 보므로, **나중에 만들어지는 탭과
  캡쳐 도구까지 자동으로 따라온다.** 새 탭이 랩되지 않는 버그가 생길 자리가 없다.
- **메뉴 체크 상태는 `populate()`가 다시 만든다.** `sync_recent_sql_file_items`가
  메뉴를 통째로 `clear()` + `populate()` 하므로, 최근 파일 목록이 바뀔 때마다 체크가
  풀리는 일을 한 곳에서 막았다.
- **래핑이 깨뜨리는 실제 로직이 하나 있어서 같이 고쳤다.**
  `rehighlight_visible_semantic_window`가 `editor.scroll_row()`를 **버퍼 줄 번호**로
  쓰고 있었다. 래핑을 켜면 그 값은 *표시 행* 번호가 되어 긴 줄이 하나만 있어도
  하이라이트 창이 화면 밖을 겨냥한다. 이제 에디터 사각형의 위/아래 끝에 있는 문자를
  물어 버퍼 줄로 바꾼다 — **래핑 여부와 무관하게 옳으므로 분기가 없다.**
- **범위를 벗어난 줄 번호는 거절이 아니라 클램프다.** 사용자는 "어딘가로 가자"고
  했으므로 가장 가까운 끝이 가장 정직한 답이다. 다만 숫자가 아닌 입력은 추측하지
  않고 사유를 말한다 — `12a`를 12로 읽어 커서를 옮기면 시키지 않은 일이 된다.

**GUI 검증**은 FLTK가 정말 랩 모드를 바꿨는지를 `count_lines`(표시 행 수)로 확인한다.
"바꾸라고 시켰다"가 아니라 "긴 줄 하나가 4행으로 그려진다"를 본다. 열려 있던 탭과
토글 이후 새로 만든 탭 양쪽, 설정 저장, 되돌리기, Go to Line의 이동/클램프/취소까지
실제 메뉴 바와 실제 모달로 확인한다.

**남은 것**: 코드 폴딩은 여전히 Tier 2다(FLTK `TextEditor`에 폴딩 개념이 없다).

#### 재검증 (2026-08-08) — 이상 없음

항목 4·11 작업 뒤 `verify_editor_convenience_ui`를 다시 돌렸다. 21개 확인이 모두
통과한다 — 랩 토글, 열려 있던 탭과 이후 만든 탭 양쪽 적용, 설정 저장, 메뉴 체크
상태, 메뉴 재생성 후에도 유지, 되돌리기, Go to Line의 이동·클램프·빈 버퍼·멀티바이트
문서·취소까지. 새로 만든 값 창과 색상 표면이 겹쳐 깨뜨린 데는 없다. 새 코드 없음.

---

## Tier 2 — 있으면 좋음. 큰 기능이거나 사용 빈도가 낮음

### 14. 라이트 테마 / 테마 선택

- **현재 상태**: 다크 팔레트 하드코딩(`src/ui/theme.rs:15` — "Windows 11-inspired
  dark palette"). 모든 색이 함수 반환 상수라 전환 스위치가 없다.
- **난이도**: 중. 팔레트를 런타임 조회 가능한 구조로 바꾸는 리팩터가 선행.
  **색상 함수 시그니처가 이미 함수 호출 형태라 내부만 교체하면 되므로,
  생각보다 기계적인 작업이다.**

### 15. 단축키 커스터마이징

- **현재 상태**: `src/ui/menu.rs`에 하드코딩. Help > Keyboard Shortcuts는 고정
  텍스트 다이얼로그다.
- **난이도**: 중(설정 저장 + 충돌 검사 + 표시 텍스트 동기화).

### 16. 미저장 편집 내용 복원 / 로컬 히스토리

- **현재 상태**: 최근 파일 목록은 있으나(`File/Recent N`), **탭 내용 자동 저장·
  복원은 없다.** 앱이 죽으면 미저장 SQL이 사라진다(크래시 리포트는 남는다).
- **왜 낮지 않을 수도 있나**: 데이터 손실이라 체감이 크다. 사용 빈도는 낮지만
  발생 시 신뢰를 잃는 종류. **구현이 작다면 Tier 1로 올릴 것.**
- **난이도**: 소~중(탭 버퍼를 주기적으로 data_dir에 스냅샷 → 시작 시 복원 제안).

### 17. 스키마 비교 / 데이터 비교 (Diff)

- **현재 상태**: 없음.
- **난이도**: 대.

### 18. 데이터 전송 (테이블 → 다른 스키마/DB 복사)

- **현재 상태**: 없음.
- **난이도**: 대. 단 임포트(3번)와 내보내기(2번)가 생기면 그 조합으로 상당 부분
  대체된다. **3, 2번을 먼저 하고 재평가할 것.**

### 19. ER 다이어그램

- **현재 상태**: 없음.
- **난이도**: 대(그래프 레이아웃 엔진 필요). FLTK 환경에서 비용이 특히 높다.

### 20. 프로시저/뷰 소스 편집 → 서버 반영 워크플로

- **현재 상태**: Generate DDL로 소스를 에디터에 가져와 `F5`로 재실행하는 방식은
  가능하다. 즉 **부분 지원**. DataGrip처럼 "객체 소스 탭 열기 → 편집 → Submit"
  전용 흐름은 없고, 컴파일 오류를 소스 라인에 매핑해 표시하지 않는다.
- **난이도**: 중(`Check Compilation`이 이미 있으므로 오류 → 라인 매핑이 핵심).


### 29. 결과 탭 고정(Pin) — 이전 결과 보존

- **현재 상태**: 새 실행마다 그 SQL 탭의 결과 그리드가 **전부 지워진다**
  (`clear_result_grids_for_new_query_batch`, `src/ui/main_window.rs:2922`).
  이전 결과와 나란히 비교하려면 SQL 탭을 하나 더 열어 거기서 실행해야 한다.
- **왜 Tier 2인가**: 우회로(탭 추가)가 명확하고 비용이 낮다. 다만 DataGrip
  사용자는 "결과를 남겨두고 다음 쿼리"를 무의식적으로 기대한다.
- **난이도**: 소~중. 고정된 탭을 clear 대상에서 빼고 표시만 하면 되지만, 고정된
  탭이 들고 있는 lazy fetch 세션의 수명이 새 배치와 겹치므로 그 정리 규칙을
  건드려야 한다(과거 회귀가 몰려 있던 영역).

### 30. SSH 터널 커넥션

- **현재 상태**: 없음(소스 전역에 `ssh` 관련 코드 없음). SSL/TCPS와 MySQL SSL CA는
  있다(`connection_dialog.rs:1131`, `1293`).
- **왜 Tier 2인가**: 배스천 뒤의 DB에 붙어야 하는 환경에서는 사실상 필수지만,
  외부 `ssh -L` 터널로 우회 가능하고 그 비용이 1회성이다.
- **난이도**: 중~대. 크레이트 도입 + 키/암호 보관(키체인) + **터널 수명을 커넥션
  수명에 묶는 일**이 핵심이고, 재연결·풀 크기와 얽힌다.

### 31. 바인드 파라미터 값 입력 프롬프트 (`:x`)

- **현재 상태**: SQL*Plus 방식은 있다 — `&var` 치환은 값을 묻고
  (`execution.rs:19331` "Enter value for …"), 바인드는 `VARIABLE` 선언
  (`ToolCommand::Var`)으로 쓴다. 하지만 애플리케이션 코드에서 복사해 온
  `... WHERE id = :id`를 그대로 실행하면 DB 에러가 난다. DataGrip은 이때 값 입력
  창을 띄운다.
- **왜 여기 있나**: 빈도는 사람에 따라 갈리지만(앱 SQL을 자주 붙여넣는 사람에겐
  매일), `VARIABLE` 선언이라는 우회로가 존재한다.
- **난이도**: 소~중. 문장에서 바인드 이름을 뽑는 판정이 이미 있다 —
  `src/ui/result_filter.rs:89`가 필터 차단용으로 같은 스캔을 한다(리터럴·q-quote
  안의 `:`을 제외하는 처리 포함). 그 판정을 재사용해 프롬프트를 띄우고 선언+대입으로
  바꾸는 형태.

#### 구현 상태 (2026-08-08) — 완료

값이 없는 플레이스홀더를 실행 직전에 찾아 모달로 묻는다. `:name` / `:1` 이름 바인드와
JDBC 철자 `?` 위치 지정자를 모두 처리한다. 파라미터마다 **타입**(String / Number /
Date / Timestamp, Oracle은 + Ref Cursor) × **값** × **NULL** 체크박스.

타입 선택이 필요한 이유는 문자열 리터럴이 값이 아니라 **문법 오류**가 되는 자리가
있기 때문이다 — Oracle `FETCH FIRST :n ROWS ONLY`, MySQL `LIMIT :n`. Date/Timestamp
값은 백엔드와 무관하게 `YYYY-MM-DD HH:MM:SS`로 쓴다. 빈 칸은 String을 제외한 모든
타입에서 SQL NULL이다(빈 숫자·빈 날짜라는 것은 없으므로).

**PL/SQL OUT 파라미터**도 같은 방식으로 답한다. 값을 비우고 타입만 고르면 되고,
OUT `SYS_REFCURSOR`는 Oracle 전용 `Ref Cursor` 타입으로 답한다(그 행의 값·NULL
컨트롤은 비활성). 즉 `BEGIN emps_by_dept(:dept, :cnt, :rc); END;`를 `VARIABLE` 선언
없이 실행하고 커서 결과까지 볼 수 있다.

**프로시저·함수 실행 방법 전부**를 커버한다.

| | 프로시저 | 함수 |
| --- | --- | --- |
| Oracle | `BEGIN p(:a,:b); END;` · `EXEC` · `EXECUTE` · `CALL` · `DECLARE … BEGIN … END;` | `SELECT f(:a) FROM DUAL` · `BEGIN :r := f(:a); END;` · `EXEC :r := f(:a)` |
| MySQL / MariaDB | `CALL p(:a, @out)` | `SELECT f(:a)` |

`EXEC`가 관건이었다 — 실행 워커 깊은 곳에서 PL/SQL 블록으로 재작성되므로, 프롬프트는
**사용자가 쓴 철자 그대로** 플레이스홀더를 찾아야 한다. MySQL의 OUT 인자는 사용자
변수여야 하는데 `@out`은 플레이스홀더가 아니라 그대로 통과하고 옆의 IN 값만 치환된다.

서버에 닿는 방식은 계열별로 갈린다.

| | 실행 방식 | SQL 텍스트 |
| --- | --- | --- |
| Oracle thin / OCI | 값을 세션 바인드로 선언해 기존 `resolve_binds` 파이프라인에 태움 | 그대로. 단 `?`는 서버가 안 받으므로 생성 이름(`:SQ_P1`…)으로 치환 |
| MySQL / MariaDB | 바인드 경로가 없어 리터럴로 치환 (`grid_sql_export::sql_literal_for_value` 재사용) | 치환된 텍스트. 히스토리에도 실제 실행된 SQL이 남는다 |

**`VARIABLE` 선언과의 관계** — 선언된 바인드는 묻지 않는다. `BindVar.prompted`
플래그로 "선언"과 "직전 답"을 구분하므로, 프롬프트가 쓴 값이 다음 실행에서 선언처럼
보여 값이 얼어붙는 일이 없다. 선언된 것과 안 된 것이 한 문장에 섞여 있으면 안 된 것만
묻고 선언된 값은 그대로 쓴다. 프롬프트 값은 매번 다시 묻되 직전 답이 채워져 있다.
취소하면 아무것도 실행되지 않는다(예약도 히스토리도 남지 않음).

- **진입점**: `execute_sql_with_mysql_delimiter_after_lazy_cancel`의 프리플라이트
  (`src/ui/sql_editor/execution.rs`). 모든 에디터 실행 경로가 여기로 모이므로
  Ctrl+Enter · 선택 실행 · F5 스크립트 · F6 실행 계획이 한 훅으로 덮인다.
- **구성**: `src/ui/bind_prompt.rs`(FLTK 없는 판정·치환 로직 + 단위 테스트 30개),
  `src/ui/bind_prompt_dialog.rs`(모달).
- **검증**: `cargo run --bin verify_bind_prompt_ui`(모달 위젯 배선 10항목),
  `cargo run --bin verify_bind_prompt_live all`(Oracle thin 36 · OCI 36 ·
  MySQL 27 · MariaDB 27 전부 통과). 다루는 축: 선언·미선언·혼합, `?`, NULL,
  Date/Timestamp, 행 제한 절, 리터럴 안 콜론, 재프롬프트, 취소, OUT 스칼라·OUT 커서,
  프로시저·함수의 모든 호출 형태, 그리고 **데이터 타입 전수**(Oracle:
  NUMBER · NUMBER(p,s) · BINARY_DOUBLE · VARCHAR2 · CHAR · NVARCHAR2 · DATE ·
  TIMESTAMP · TIMESTAMP WITH TIME ZONE · CLOB · RAW / MySQL 계열: INT · BIGINT ·
  DECIMAL · DOUBLE · VARCHAR · CHAR · TEXT · DATE · DATETIME · TIMESTAMP · TIME ·
  BLOB · JSON). 값이 왕복에서 깨지면 행이 안 걸리므로 빈 결과가 곧 실패다.
- **README**: `#### Bind parameter values` + `docs/images/bind-parameters.png`.

**덤으로 고친 기존 결함 (2건, 둘 다 Oracle thin. OCI는 둘 다 영향 없음)**

1. **TIMESTAMP 바인드가 같은 값과 비교되지 않았다.** 초 미만이 0인 TIMESTAMP를
   11바이트(뒤 4바이트가 0)로 보냈다. Oracle은 같은 값을 7바이트로 저장하고 **저장
   형태로 비교**하므로(`DUMP` 결과 `Typ=180 Len=7`) `col = :bind`가 명백히 같은
   값에도 거짓이었다. 나노초가 0이면 7바이트로 인코딩하도록 고쳤다 — OSON 인코더는
   이미 같은 규칙을 쓰고 있었다(`encode_oson_timestamp_json`).
2. **PL/SQL OUT 바인드 결과가 한 칸씩 밀렸다.** 값 있는 IN 바인드와 섞이면 엉뚱한
   바인드에 배정됐다. 서버는 **문장의 실제 파라미터 모드**로 답하므로 IN OUT으로
   보낸 바인드라도 `IN` 파라미터면 되돌아오지 않는데, 앱은 위치로만 짝지었기
   때문이다(`p(:dept,:cnt,:rc)` → `:DEPT`에 count, `:CNT`에 커서). 드라이버가
   `OutBindResult.value_bind_indices`로 바인드 인덱스를 함께 돌려주도록 하고
   그것으로 짝짓는다. `VARIABLE dept NUMBER; EXEC :dept := 30;
   EXEC p(:dept,:cnt,:rc)`로 예전에도 재현되던 결함이다.

### 32. FK 기반 JOIN 조건 자동완성

- **현재 상태**: 없음. IntelliSense는 테이블/컬럼/키워드까지 하고, `JOIN t2 ON `
  슬롯에서 외래키로 조인 조건을 제안하지는 않는다.
- **왜 여기 있나**: 13번(FK로 관련 데이터 이동)과 **같은 메타데이터**를 쓴다.
  두 항목을 묶으면 FK 메타데이터 로딩·캐시를 한 번만 만들면 된다.
- **난이도**: 중.

### 33. 세션 강제 종료 (Session Activity에서)

- **현재 상태**: Session Activity는 조회 전용이다. `KILL QUERY` / `KILL CONNECTION`
  은 **자기 쿼리를 취소하는 경로에만** 있다(`src/ui/sql_editor/mod.rs:1812`).
- **왜 낮은가**: 개인 개발자에게는 드물고, 남의 세션을 끊는 것은 파괴적이다.
  넣는다면 5번과 같은 **미리보기 후 실행** 형태여야 한다.
- **난이도**: 소(문장 자체) + 확인 UI와 권한 부족 시의 에러 처리.

---

## Tier 3 — 범위 밖 또는 전략적 판단 필요

- **21. 추가 DB 지원(PostgreSQL, SQL Server, SQLite 등)**: DataGrip의 최대
  차별점이지만 이 제품은 Oracle/MySQL/MariaDB로 의도적으로 좁혀져 있다.
  기능 격차라기보다 제품 포지셔닝 결정 사항.
- **22. VCS 연동, 플러그인/스크립트 확장(Groovy extractor 등)**: 개인 사용자
  필수 기능 아님.
- **23. 데이터베이스 콘솔 다중 창/분할 뷰**: 사용 빈도 대비 비용 높음.
- **기타(항목으로 세우지 않음)**: SQL 파일 인코딩 선택, 인쇄, 코드 폴딩(28번에
  적어둔 대로 FLTK에서 비용이 크다), 트리에서 시스템 객체 표시 토글. 재조사에서
  전부 없는 것을 확인했지만, 없어서 막히는 흐름이 떠오르지 않아 목록에 세우지 않았다.

---

## 권장 착수 순서

작은 비용으로 체감이 큰 것부터 묶었다.

| 순서 | 항목 | 근거 |
| --- | --- | --- |
| ✅ | ~~2. 내보내기 포맷 확장~~ | **완료** (Excel 제외) |
| ✅ | ~~1. 결과 필터 (서버 재조회)~~ | **완료** — 단 GUI 실사용 확인이 남음 |
| ✅ | ~~11-b. 그리드 내 텍스트 검색~~ | **완료** |
| ✅ | ~~5. Drop / Truncate~~ | **완료** — Rename/Modify Table은 범위 밖 |
| ✅ | ~~7. 선택 집계 표시~~ · ~~8. 코드 스니펫~~ | **완료** |
| ✅ | ~~3. 파일 임포트~~ | **완료** — UI 연결까지 |
| ✅ | ~~28. 소프트 랩 · Go to Line~~ | **완료** — 폴딩은 범위 밖 |
| ✅ | ~~10. Go to Declaration / 전역 객체 검색~~ | **완료** — 서버 전역 검색은 범위 밖 |
| ✅ | ~~12. 실행 계획 시각화~~ | **완료** — MySQL 계열은 트리 없이 서버 컬럼 그대로 |
| ✅ | ~~31. 바인드 파라미터 값 입력 프롬프트~~ | **완료** — `:name` · `:1` · `?`, 4백엔드 라이브 검증 |
| ✅ | ~~4. 값 뷰어/에디터 패널~~ | **완료** — Oracle 긴 값·CLOB 편집까지 뚫음. Transpose는 범위 밖 |
| ✅ | ~~11. 커넥션 색상 + 읽기 전용~~ | **완료** — 클라이언트 가드, 4백엔드 라이브 검증 |
| 1 | 25. 셀 값으로 빠른 필터 | 이미 있는 `grid_sql_export` 리터럴 생성을 그대로 쓴다 |
| 2 | 26. 객체 트리 Export / Import | 3번이 끝난 지금 메뉴 항목 하나로 끝난다 |
| 3 | 24. 결과 정렬 수단 | 헤더 정렬 제거로 생긴 구멍. (a)안이면 작다 |
| 4 | 16. 미저장 탭 복원 | 데이터 손실 방지 |
| 5 | 6. 컬럼 숨김/순서 · 27. 트리 컬럼 노드 | 구조 변경 수반 |

### 이 목록의 한계 (검토 요청)

- 시급도는 **일반 개발자 사용자**를 기준으로 매겼다. 대상 사용자가 DBA 쪽이면
  실행 계획 시각화(12)와 스키마 비교(17)가 훨씬 위로 올라간다.
- 난이도는 소스 구조를 읽고 추정한 값이며 실측이 아니다. 특히 3(임포트)과
  6(컬럼 숨김)은 기존 컬럼 인덱스 가정과 충돌할 수 있어 추정보다 커질 수 있다.
- 20번(프로시저 편집 워크플로)은 "부분 지원"이라 완전 미구현 항목들과 성격이
  다르다. 실제 사용 흐름을 한 번 확인한 뒤 순위를 정하는 편이 낫다.
- 2026-08-08 재조사는 **진입점이 있는가**만 봤다. 24·25·26·28번은 "없다"가
  확실하지만(해당 문자열·핸들러가 소스에 아예 없다), 27·29번은 지금 구조에서
  얼마나 커지는지가 추정이다.
- 24번은 다른 항목과 성격이 다르다. **없던 기능이 아니라 있다가 뺀 기능**이라,
  다시 넣을지 자체가 제품 결정이다. 여기서는 구멍의 위치만 적었다.

---

## 부록 A. 일반 쿼리 결과 필터 구현 계획

1번 항목의 상세. **방식: 서버 재조회(파생 테이블 래핑).** 로컬 평가 방식은
1번 본문의 사유로 기각.

### A.1 재사용 가능한 것

`TableBrowseTarget.relation_sql`은 테이블명이 아니라 **`FROM` 뒤에 놓이는 자유
관계식**이다:

```rust
// src/ui/table_browse.rs:233
let mut sql = format!("SELECT * FROM {}", target.relation_sql);
if !clauses.where_expr.is_empty()    { sql.push_str("\nWHERE ");    ... }
if !clauses.order_by_expr.is_empty() { sql.push_str("\nORDER BY "); ... }
```

`relation_sql`에 `SCOTT.EMP` 대신 `(<원본 쿼리>) sq_src`를 넣으면 아래가 전부
그대로 동작한다:

| 재사용 대상 | 위치 |
| --- | --- |
| 논리 SQL 조립 | `build_logical_sql` (`table_browse.rs:226`) |
| 총 건수 쿼리 | `build_count_sql` (`table_browse.rs:245`) |
| 페이징 (Oracle ROWNUM / MySQL LIMIT OFFSET) | `build_page_sql` (`table_browse.rs:261`) |
| 다중 문·툴 커맨드 차단 | `validate_single_statement` (`table_browse.rs:216`) |
| 필터 입력창 + IntelliSense | `TableBrowseFilterBar` (`table_browse.rs:325`) |
| 읽기 전용 전환 | `TableBrowseTarget::read_only()` (`table_browse.rs:78`) |

Oracle 정렬-후-페이징 순서도 이미 맞다. `build_page_sql`이 ORDER BY가 포함된
`logical_sql`을 `sq_page_source`로 감싼 **뒤** ROWNUM을 적용한다.

### A.2 반드시 필요한 게이트 4개

**게이트 1 — 문 종류. `is_select_like`를 쓰면 안 된다.**
`sql_classification.rs:338`이 `DESCRIBE | DESC | SHOW`를 `SelectLike`로 분류하는데,
이들은 MySQL에서 파생 테이블이 될 수 없다. 별도의 `is_wrappable_relation` 판정
(SELECT / WITH…SELECT / VALUES / TABLE)이 필요하다. 함께 제외할 것:
바인드·치환 변수(`:x`, `&x`)가 든 SQL, 스크립트 산물(PRINT / ref cursor /
DBMS_OUTPUT), 미완료 상태의 문.

**게이트 2 — 별칭과 정규화.**
MySQL/MariaDB는 파생 테이블 별칭이 필수(ERROR 1248)이고 Oracle은 테이블 별칭에
`AS`를 거부한다. 따라서 **`(...) sq_src` (AS 없는 별칭)** 이어야 양쪽을 만족한다.
원본 SQL의 후행 `;`는 제거한다 — `compose_edit_script`가 이미
`.trim_end_matches(';')`로 같은 처리를 한다(`result_table.rs:5328`).

**게이트 3 — 중복 컬럼명. DB마다 동작이 다르다 (실측 완료, 부록 C).**

조사 초기의 추정("Oracle ORA-00918 / MySQL 1060으로 똑같이 깨진다")은 **틀렸다.**
실측 결과 두 계열이 갈린다:

- **Oracle**: 중복 컬럼이 있어도 **감싸는 것 자체는 성공한다.** 페이징 래핑까지
  포함해 정상 동작. 실패는 WHERE/ORDER BY가 **중복된 이름을 지목할 때만**
  일어나고, 에러는 ORA-00918이 아니라 **ORA-00904 "invalid identifier"** 다.
  중복되지 않은 컬럼으로 거는 필터는 잘 동작한다.
- **MySQL / MariaDB**: **파생 테이블 생성 자체가 거부된다** (ERROR 1060).
  필터가 중복 컬럼을 건드리는지와 **무관하게** 실패한다.

따라서 게이트를 DB별로 나눈다:

- MySQL/MariaDB → 결과 컬럼 이름에 중복이 있으면 **필터 기능 자체를 비활성화**
  하고 이유를 표시한다.
- Oracle → 필터를 **허용**하되, 중복된 이름을 참조하면 ORA-00904가 나므로
  해당 컬럼명을 미리 알고 안내한다.

판정은 드라이버가 준 `QueryResult.columns`의 이름 중복만 보면 실행 전에 가능하다.
DB 에러를 그대로 노출하는 일은 없어야 한다.

**게이트 4 — 재실행임을 드러낼 것.**
테이블 브라우징의 재조회는 `SELECT * FROM tbl`이라 싸고 부작용이 없다. 임의
사용자 쿼리는 (a) 비쌀 수 있고 (b) 쓰기를 하는 함수를 호출할 수 있고 (c) 화면과
다른 데이터를 낼 수 있다. "몰래 아무것도 하지 않는다"는 원칙상 **필터 적용 =
쿼리 재실행임이 보여야** 한다.

주입 위험은 새로 생기지 않는다. WHERE 입력이 사용자 SQL인 것은 설계 의도이고
`validate_single_statement`가 이미 다중 문을 막는다. 테이블 브라우징과 동일한
수준.

### A.3-a 구현 상태 (2026-08-07) — 완료

필터 바는 **결과가 도착할 때 기본으로 노출**된다. 단 재조회할 수 없는 결과
(게이트가 `Blocked`)에는 아예 붙지 않으므로 바가 보이지 않는다.

| 조각 | 위치 |
| --- | --- |
| 게이트 3개 | `src/ui/result_filter.rs` (테스트 26개) |
| 필터 바 부착 (행 보존) | `ResultTabs::attach_result_filter_bar_by_id` |
| 적용 시 탭 종류 승격 | `ResultTabs::promote_query_tab_to_table_browse` |
| 게이트 판정 + 부착 | `MainWindow::offer_result_filter` (SelectStart에서 호출) |
| 파생 관계 컬럼 완성 | `TableBrowseTarget::result_columns` + `merge_filter_suggestions` |
| ~~헤더 정렬 리다이렉트~~ | **제거됨** — 헤더 정렬 자체가 사라졌다. [부록 B](#부록-b-헤더-정렬-제거됨) |

자동 노출에서 반드시 꺼야 하는 두 가지가 있다. **포커스를 가져가면 안 되고**
(쿼리를 실행할 때마다 에디터에서 캐럿을 뺏는다) **상태바를 덮어써도 안 된다**
(실행 상태 메시지를 가린다). `attach_result_filter_bar_by_id`의 `focus_input`
인자가 전자를 제어한다.

**설계 변경 — 탭 종류 전환 시점을 늦췄다.** 원래 계획은 필터 바를 붙일 때
탭을 `TableBrowse`로 바꾸는 것이었는데, 구현 중에 이게 위험하다는 걸 확인했다.
`execute_table_browse_request`(`main_window.rs:8291`)와 결과 라우팅
(`main_window.rs:9591`)이 **탭 종류로 분기**하므로, 브라우즈 페이지가 아닌
일반 statement 결과가 그 탭에 도착하면 페이지 로드 경로를 타게 된다. 과거
회귀와 같은 부류다.

그래서 두 시점을 분리했다:
- **필터 바 부착** → 탭은 `Query` 그대로. 위젯만 올라가고 화면의 행과 상태는
  그대로 유지된다. 그리드 편집도 계속 가능하다.
- **필터 적용** → 이때 `TableBrowse`로 승격. 실제로 페이지 쿼리가 나가는
  순간이므로 탭 상태와 실제 동작이 일치한다.

덕분에 부작용도 하나 사라졌다. 부착 시점에 승격했다면 **모든 필터 가능한
SELECT 결과에서 그리드 편집이 죽었을 것**이다(브라우즈 타깃이 read-only라서).
지금은 필터를 실제로 걸기 전까지 편집이 살아 있다.

**추가로 잡은 버그 (1) — 줄 주석이 닫는 괄호를 먹는다.** 원본 SQL이 줄 주석으로
끝나면 `(SELECT * FROM t -- note) sq_src` 에서 `)` 가 주석에 들어간다. 닫는
괄호를 새 줄에 두는 형태로 고치고, 프로브에 `trailing_line_comment` /
`trailing_block_comment` 케이스를 추가해 세 DB 모두에서 재확인했다.

**추가로 잡은 버그 (2) — 필터 바에 컬럼 추천이 하나도 안 떴다.** 사용자 제보로
발견. 필터 바는 `completion_tables()`가 준 **테이블 이름으로 메타데이터를
조회**해서 컬럼을 찾는데(`intellisense.rs:1920`), 처음 구현에서 타깃을
`table_name: "Result"`, `completion_name: ""` 로 만들어 조회가 전부 빗나갔다.

이름으로는 풀 수 없는 문제다 — 감싼 결과는 파생 테이블이라 **조회할 이름 자체가
없고**, 그 컬럼이 존재하는 유일한 곳은 화면의 결과 헤더다. `TableBrowseTarget`에
`result_columns`를 추가해 결과 헤더를 직접 싣고, 필터 바가 그걸 먼저 매칭한 뒤
기존 엔진 결과(키워드 등)를 이어 붙이도록 했다.

테이블 브라우징 경로는 `result_columns`가 비어 있어 완전히 그대로다 —
`merge_filter_suggestions`가 그 경우 엔진 답을 손대지 않고 통과시키는 것을
테스트로 고정했다. 깨진 지점이 조합부였으므로 병합 로직도 순수 함수로 분리해
테스트했다(중복 제거·순서·상한).

검증: lib 7238개, `db_dispatch_guards` 71개, clippy 경고 0, 프로브 18케이스 ×
3 DB 모두 "No surprises".

**남은 것**: 필터 바 부착/승격은 FLTK 위젯이 필요해 유닛 테스트가 없다. 실제
GUI에서의 동작 확인이 아직이다.

### A.3 새로 만들어야 하는 것 (원래 계획)

**(1) 탭 종류 전환.** `ResultTabKind::TableBrowse(_)`는 탭 생성 시점에 결정되고
`ensure_table_browse_tab_by_id`는 이미 구성된 탭이면 그냥 반환한다
(`result_tabs.rs:1666`). **살아있는 statement 탭에 필터 바를 붙이는 경로**가
새로 필요하다.

**(2) 페칭 모델 전환.** 일반 결과는 lazy-fetch 커서를 유지하고, 브라우징은
페이지마다 바운드 쿼리를 던지며 커서를 유지하지 않는다. 필터를 걸면 모델이
바뀌므로 **전환 시점에 기존 커서를 확실히 정리**해야 한다. 이 근처는 과거
회귀가 있던 영역이다(배치 완료가 새 요청의 결과 탭을 덮어쓴 문제, 취소된 lazy
fetch의 히스토리 처리).

**(3) 헤더 정렬과의 정합성.** *(2026-08-07 종결 — 헤더 정렬을 제거하는 것으로
끝났다. 아래는 그 이전의 계획.)*
기존 헤더 정렬은 **로컬**이고 `column_kinds`를 쓰지 않는다. 서버 ORDER BY가
붙으면 두 정렬이 공존해 사용자가 어느 쪽이 적용됐는지 알 수 없게 된다.
**권장: 필터 바가 활성인 탭에서는 헤더 클릭이 로컬 정렬 대신 ORDER BY 필드를
설정해 서버 정렬로 가도록 리다이렉트한다.**

헤더 정렬 자체의 결함과 개선안은 [부록 B](#부록-b-헤더-정렬-개선)로 분리했다.
부록 A와 **함께 진행**한다.

**(4) 읽기 전용 처리.** 필터 적용 후 그리드는 read-only여야 한다. 브라우징의
editable 경로는 `maybe_inject_rowid_for_editing`으로 ROWID를 주입하는데 인라인
뷰에 이걸 하면 안 된다. `read_only()`를 쓴다.

**(5) 원본 SQL 보존.** SQL 내보내기의 base table 추론이 래퍼 SQL을 보면 안 되므로,
`source_sql`(`result_table.rs:306`)과는 별도로 원본을 유지한다.

### A.4 단계와 검증

| 단계 | 내용 | 검증 |
| --- | --- | --- |
| 1 | `is_wrappable_relation` + 래핑 SQL 조립(게이트 1·2·3)을 **순수 함수**로 구현 | DB 불필요. 유닛 테스트로 방언별 산출 SQL과 거부 케이스 고정 |
| 2 | 중복 컬럼 사전 감지 | `QueryResult.columns` 기반 유닛 테스트 |
| 3 | statement 탭 → 필터 바 부착 + 커서 정리 | 기존 lazy-fetch/배치 라우팅 회귀 테스트 |
| 4 | ~~헤더 정렬 리다이렉트~~ | **취소** — 헤더 정렬이 제거되어 리다이렉트할 대상이 없다 |
| 5 | 라이브 확인 | Oracle(thin/OCI) · MySQL · MariaDB |

1·2단계가 DB 없이 끝나므로 **여기부터 시작하는 것이 위험이 가장 낮다.**

### A.5 실측 완료

세 항목 모두 `src/bin/verify_derived_table_wrap.rs`로 Oracle 26ai(thin) ·
MySQL 8.0.46 · MariaDB 12.2.2에서 확인했다. 결과는 [부록 C](#부록-c-실측-결과).
추정과 달랐던 부분은 게이트 3에 반영했다.

---

## 부록 B. 헤더 정렬 (제거됨)

> **2026-08-07 종결 — 헤더 정렬 기능 자체가 제거됐다** (커밋 `f635179`,
> 사용자 요청). 유일한 트리거였던 컬럼 헤더 클릭과 함께 정렬 상태·정렬 표시자·
> ORDER BY 리다이렉트·`src/ui/grid_sort.rs`(비교 로직 전체)가 사라졌다. 헤더
> 누르기는 셀 드래그 선택으로 변하지 않도록 여전히 삼켜진다.
> **지금 결과를 정렬하는 방법은 필터 바의 `ORDER BY` 입력뿐이다.**
> `DatabaseType::sorts_nulls_last_ascending`은 테스트와 함께 남아 있지만 UI에서
> 읽는 곳이 없다.
>
> 남는 실질적 함의는 둘이다. (a) B.2의 결함 1~4는 코드와 함께 사라졌다.
> (b) **필터 바가 붙지 않는 결과에는 이제 정렬 수단이 전혀 없다** — 새 항목
> [24번](#24-결과-정렬-수단-헤더-정렬-제거-이후)으로 옮겼다.
>
> 아래는 제거 이전의 기록이며, 되살릴 경우의 설계 근거로만 남긴다.

부록 A와 함께 진행. 아래 1~3번은 **서버 ORDER BY와 무관하게 지금도 틀린 값을
내므로** 독립적으로 고칠 가치가 있다.

### B.1 먼저, 이미 제대로 되어 있는 것

조사 중 오해했다가 정정한 부분이다. 헤더 정렬은 **부분 fetch 상태에서 정렬하지
않는다**:

- lazy fetch가 살아 있으면 `LazyFetchPendingAction::HeaderSort`로 큐잉해
  전체 fetch 완료 후에 정렬한다 (`result_table.rs:2844` → `8844`).
- 스트리밍 중에는 아예 막는다 (`streaming_in_progress` 검사, `result_table.rs:2853`).
- 편집 중 staged row state를 행과 짝지어 함께 옮긴다 (`sort_row_entries`).

즉 **행 집합의 완전성은 이미 보장된다.** 문제는 오직 비교 함수다.

### B.2 결함

전부 `compare_row_values_for_sort` (`result_table.rs:768`)에 있다. 바로 옆
필드에 `column_kinds`(`result_table.rs:285`)가 있는데도 쓰지 않고
"f64 파싱 → 실패하면 바이트 비교"만 한다.

| # | 결함 | 영향 | 우선순위 |
| --- | --- | --- | --- |
| 1 | **Temporal을 문자열로 비교** | 아래 정정 참조. 앱 기본 설정에서는 증상이 없고, 사용자가 NLS 포맷을 바꾸거나 비웠을 때만 발생 | P2 |
| 2 | **f64 정밀도** | Oracle `NUMBER`는 38자리, f64는 15~17자리. 20자리 ID는 서로 다른 값이 뭉개져 순서가 임의로 결정됨. 에러 없이 조용히 틀림 | P1 |
| 3 | **NULL 위치** | 빈 값이 오름차순 맨 앞. Oracle 기본은 NULLS LAST라 어긋남(MySQL과는 우연히 일치). `null_text`로 표시된 NULL은 `value_represents_null`을 참조하지 않아 리터럴 텍스트로 정렬 | P2 |
| 4 | **collation** | 바이트 비교라 `Z < a`. MySQL 기본 collation은 대소문자 무구분이라 서버와 다름 | P3 (문서화) |
| 5 | **브라우징 탭에서 페이지 단위 정렬** | 헤더 정렬은 현재 페이지만, 같은 탭의 ORDER BY 필드는 전역. **새 기능이 만드는 모순이 아니라 현재도 존재** | P1 |

> **정정 (구현 중 확인).** 결함 1을 처음에 "가장 눈에 띄는 P1"으로 적었는데
> 과장이었다. 근거로 든 `DD-MON-RR`은 **Oracle 데이터베이스의** 기본값이고,
> 이 앱은 연결 시 NLS 날짜 포맷을 `yyyy-mm-dd hh24:mi:ss`로 지정한다
> (`ConnectionAdvancedSettings::default_oracle_nls_date_format`,
> `src/db/connection.rs:483`). ISO 렌더링은 문자열 정렬로도 시간순이 맞으므로
> **기본 설정에서는 증상이 나타나지 않는다.** 실제로 문제가 되는 경우는
> 사용자가 연결 대화상자에서 포맷을 바꿨을 때(예: `YYYY/MM/DD HH24:MI:SS`)나
> 설정을 비워 DB 기본값이 나올 때다. P2로 내린다.
>
> 조용히 틀리면서 기본 설정에서도 발생하는 결함 2가 이 목록의 유일한 P1이다.

### B.3 개선안 — 하이브리드

로컬 정렬을 서버와 완전히 일치시키려는 시도는 1번 항목에서 기각한 "미묘하게
다른 평가기" 함정과 같다. 다만 **정렬은 필터보다 표면적이 훨씬 작다** — 함수도
3값 논리도 암묵 형변환도 없고 타입별 순서 + NULL 위치 + collation뿐이다.
그래서 전부 포기할 필요는 없고, 경계를 이렇게 긋는다:

**(a) 재조회 가능한 탭 → 헤더 클릭을 서버 ORDER BY로 리다이렉트.**
필터 바가 있거나 원본이 래핑 가능한 경우. 정의상 정확하고 결함 5번도 함께
해소된다. DataGrip과 같은 방식.

**(b) 재조회 불가능한 탭 → 로컬 정렬 유지 + 결함 1·2·3 수정.**
스크립트 산물, 래핑 불가 문, 연결 끊김 상태. `column_kinds`로 분기:

- `Number` → f64 대신 **문자열 기반 십진 비교**(부호 · 정수부 길이 · 자릿수 순).
  정밀도 손실 없음.
- `Temporal` → **세션 NLS 포맷으로 파싱**해 비교. 파싱 실패 시 문자열 비교로
  안전하게 강등.
- NULL → `value_represents_null`로 판정하고 **DB별 기본 위치**를 따름
  (Oracle NULLS LAST / MySQL NULLS FIRST, 오름차순 기준).
- `String` / `Unknown` / `Binary` → 현행 유지.

**(c) collation은 흉내내지 않는다.** MySQL collation 종류가 많고 Oracle
`NLS_SORT`도 별개라 정확히 맞출 수 없다. **"로컬 정렬은 이진 비교"임을 명시**하고,
정확한 정렬이 필요하면 (a) 경로를 쓰게 한다.

### B.4 구현 상태 — 고쳤다가 기능째 제거됨

> **후속 (2026-08-07).** 아래 수정은 실제로 들어갔다가, 같은 날 헤더 정렬이
> 제거되면서 `src/ui/grid_sort.rs`와 함께 통째로 삭제됐다. 되살리려면 커밋
> `f635179`의 역방향에서 파일을 그대로 꺼내면 된다(순수 함수 + 테스트 27개라
> 되살리는 비용은 작다).

**결함 1·2·3 수정 완료.** `src/ui/grid_sort.rs`(신규, 순수 함수, 테스트 27개)에
비교 로직을 두고 `result_table.rs`의 `compare_row_values_for_sort` →
`sort_row_entries` → `apply_sort_to_table_data` 경로에 연결했다.

- **결함 2**: f64를 버리고 부호·정수부 길이·자릿수를 직접 비교. 지수 표기 등
  이해하지 못하는 형태는 텍스트 비교로 강등한다.
- **결함 1**: 연도 우선(`-` 또는 `/` 구분) 과 `DD-MON-RR`/`DD-MON-YYYY`(RR 규칙
  포함)를 파싱. `MM/DD/YYYY`처럼 `DD/MM/YYYY`와 구분 불가능한 형태는 추측하지
  않고 텍스트 비교로 강등하므로, 기존 동작보다 나빠지는 경우가 없다.
- **결함 3**: `DbBackend::sorts_nulls_last_ascending()`을 추가하고(Oracle true,
  MySQL/MariaDB false) 결과 탭 생성 시 `set_sort_null_ordering_by_id`로 그리드에
  주입한다. 백엔드를 알 수 없는 경로는 기존 동작(`FirstOnAscending`)을 유지한다.
  `src/ui`에서 `match db_type`을 금지하는 가드 때문에 db 레이어의 스펙 메서드로
  구현했고, 새 백엔드가 추가되면 컴파일 에러가 난다.
- **결함 4(collation)**: 예정대로 흉내내지 않는다. 텍스트는 바이트 비교.

검증: lib 테스트 7238개 통과, `db_dispatch_guards` 71개 통과, clippy 경고 0.

~~**남은 것 — 결함 5**는 헤더 정렬을 서버 ORDER BY로 리다이렉트하는
부록 A.3(3)과 같은 작업이라 그쪽에서 함께 처리한다.~~ → 결함 5는 헤더 정렬이
사라지면서 함께 없어졌다(페이지 단위 정렬을 하는 코드가 없다).

---

## 부록 C. 실측 결과

측정일 2026-08-06. 프로브: `src/bin/verify_derived_table_wrap.rs`
(`cargo run --bin verify_derived_table_wrap <oracle|mysql|mariadb>`).
컨테이너는 메모리 때문에 **하나씩** 띄워 확인했다.

- Oracle 26ai Free (thin, 컨테이너 `oracle`, 127.0.0.1:1521 FREE)
- MySQL 8.0.46 (컨테이너 `space-query-mysql80`, 127.0.0.1:3307)
- MariaDB 12.2.2 (컨테이너 `space-query-mariadb122`, 127.0.0.1:3306)

프로브는 `table_browse.rs`의 `marked_materialized_sql` / `build_logical_sql` /
`build_page_sql` SQL 형태를 그대로 옮겨 쓰므로, 결과가 실제 구현 SQL에 그대로
적용된다. (해당 함수들이 `pub(crate)`라 별도 크레이트인 bin에서 직접 호출할 수
없어 전사했다. 실제 빌더가 파생 관계식을 받게 되면 직접 호출로 바꿀 것.)

### C.1 결과표

| 케이스 | Oracle | MySQL 8 | MariaDB 12.2 |
| --- | --- | --- | --- |
| `(...) sq_src` — AS 없는 별칭 | OK | OK | OK |
| `(...) AS sq_src` | **거부** ORA-03048 | OK | OK |
| 별칭 없음 | OK | **거부** 1248 | **거부** 1064 |
| 중복 컬럼 조인을 감싸기만 | **OK** | **거부** 1060 | **거부** 1060 |
| 중복 컬럼 + WHERE가 중복 이름 지목 | **거부** ORA-00904 | 거부 1060 | 거부 1060 |
| 중복 컬럼 + WHERE가 정상 이름 지목 | **OK** | **거부** 1060 | **거부** 1060 |
| 중복 컬럼 + ORDER BY가 중복 이름 지목 | 거부 ORA-00904 | 거부 1060 | 거부 1060 |
| 중복 컬럼 + 전체 페이징 래핑 | **OK** | 거부 1060 | 거부 1060 |
| 명시적 중복 별칭 (`1 AS X, 2 AS X`) | 거부 ORA-00918 | 거부 1060 | 거부 1060 |
| 파생 테이블 안의 `WITH` | OK | OK | OK |
| 같은 것 + WHERE/ORDER BY | OK | OK | OK |
| `UNION ALL` 원본 + WHERE/ORDER BY | OK | OK | OK |
| 원본이 `ORDER BY`로 끝남 | OK | OK | OK |
| 전체 페이징 래핑 (+1 lookahead) | OK | OK | OK |

### C.2 확정된 사실

1. **별칭은 `(...) sq_src` 형태로 고정.** Oracle이 `AS`를 거부하고(ORA-03048)
   MySQL 계열이 별칭 없음을 거부하므로, AS 없는 별칭만이 셋 다 통과하는 유일한
   형태다. 게이트 2의 근거가 실측으로 확정됐다.

2. **중복 컬럼 처리는 DB별로 갈린다** — 추정이 틀렸던 부분. Oracle은 감싸기와
   페이징까지 정상이고 중복 이름을 *참조할 때만* ORA-00904로 실패한다. MySQL과
   MariaDB는 파생 테이블 생성 단계에서 1060으로 거부하며, 필터가 그 컬럼을
   건드리는지와 무관하다. **Oracle 사용자는 조인 결과에도 필터를 쓸 수 있고,
   MySQL 계열은 못 쓴다.** 게이트 3을 DB별로 나눈 이유다.

3. **`WITH`는 세 DB 모두 파생 테이블 안에서 허용된다.** 게이트 1에서 CTE 쿼리를
   제외할 필요가 없다.

4. **`UNION ALL` 원본과 `ORDER BY`로 끝나는 원본 모두 안전하게 감싸진다.**
   집합 연산 결과에 필터를 거는 것은 이 기능의 주요 이점 중 하나인데, 실측으로
   확인됐다.

5. **페이징 래핑과 정렬이 함께 정확하다.** `page_size 2 / offset 1`을
   `ORDER BY NAME`으로 실행해 Oracle·MySQL·MariaDB 모두 `bravo, charlie, delta`
   3행(= page_size + 1 lookahead)을 정확한 순서로 반환했다. Oracle ROWNUM 래핑이
   정렬을 먼저 적용한다는 전제가 실측으로 확인됐다.

### C.3 남은 한계

- Oracle은 **thin만** 확인했다. 방언 수용 여부는 서버 속성이라 드라이버와
  무관하지만, OCI 경로는 확인하지 않았다.
- 프로브는 SQL 수용/거부와 결과 순서만 본다. 성능(래핑이 실행 계획에 미치는
  영향)은 측정하지 않았다.
- MySQL 계열의 1060 제약을 우회하는 방법(투영 재작성)은 시도하지 않았다.
  원본 SQL 파싱이 필요해 위험 대비 이득이 없다고 판단했다.
