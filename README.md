# SPACE Query

SPACE Query는 Rust와 FLTK로 만든 데스크톱 SQL 클라이언트입니다. Oracle, MySQL, MariaDB 연결을 지원하고, SQL 편집기, 스크립트 실행, 오브젝트 브라우저, 결과 그리드, 쿼리 히스토리, 세션 활동 보기, 로그/크래시 진단 기능을 한 앱 안에 묶습니다.

## 지원 데이터베이스

- Oracle
  - Thin 모드(내장 TNS 클라이언트)와 OCI(thick) 모드 선택 지원
  - Thin 모드는 Oracle Instant Client 없이 연결, OCI 모드는 Instant Client 또는 Full Client 필요
  - Host/Port/Service 연결과 TNS alias 연결 지원
  - TCP/TCPS, NLS date/timestamp format, session time zone, 기본 트랜잭션 옵션 설정 지원
- MySQL
  - 데이터베이스 선택, SSL 옵션, SQL mode, charset/collation, session time zone 설정 지원
- MariaDB
  - MySQL 계열 실행 백엔드와 SQL dialect를 공유하되 별도 데이터베이스 종류로 구분 관리
  - MariaDB time zone 범위와 일부 메시지 처리를 분리

## 주요 기능

### 연결과 세션

- 저장된 연결 목록 관리
- 비밀번호는 설정 파일이 아니라 OS Keyring에 저장
- 연결별 고급 옵션 검증
- 최근 SQL 파일 목록 유지
- 연결 풀 기반 세션 취득
- 긴 조회를 위한 lazy fetch와 결과 탭별 세션 추적
- 연결 전환, disconnect, commit/rollback 전 실행 중 쿼리와 lazy fetch 상태 확인
- Tools > Session Activity에서 현재 연결/풀/결과 탭 상태 확인

### SQL 편집기

- 다중 SQL 탭
- 새 SQL 파일, 열기, 저장, 다른 이름으로 저장, 닫기
- 문법 하이라이팅
- IntelliSense 팝업
- Find / Replace
- SQL 포맷
- 주석 토글
- 선택 영역 대문자/소문자 변환
- 현재 문장 선택과 실행
- 선택 영역 실행
- 스크립트 전체 실행
- 실행 타임아웃 입력
- 쿼리 히스토리 이전/다음 탐색
- 커서 위치 Quick Describe

### 실행

- `F5`: 스크립트 실행
- `Ctrl+Enter`, `F9`: 현재 문장 실행
- `F4`: Quick Describe
- `F6`: Explain Plan / EXPLAIN
- `F7`: Commit
- `F8`: Rollback
- Tools > Auto-Commit 토글
- Oracle bind variable, `PRINT`, ref cursor 결과 처리
- MySQL/MariaDB `SHOW`, `DESC`, `EXPLAIN` 결과셋 처리
- 실행 결과와 메시지를 결과 탭으로 분리 표시

### 스크립트와 툴 명령

스크립트 실행은 단순 세미콜론 분리가 아니라 전용 파서와 세션 상태를 함께 사용합니다.

Oracle / SQL*Plus 계열:

- `VAR`, `VARIABLE`, `PRINT`
- `SET SERVEROUTPUT`
- `SET DEFINE`, `SET SCAN`, `SET VERIFY`, `SET ECHO`, `SET TIMING`, `SET FEEDBACK`, `SET HEADING`
- `SET PAGESIZE`, `SET LINESIZE`, `SET TRIMSPOOL`, `SET TRIMOUT`, `SET SQLBLANKLINES`, `SET TAB`, `SET COLSEP`, `SET NULL`
- `SHOW ERRORS`, `SHOW USER`, `SHOW ALL`
- `DESC`, `DESCRIBE`
- `PROMPT`, `PAUSE`, `ACCEPT`
- `DEFINE`, `UNDEFINE`, `COLUMN ... NEW_VALUE`
- `BREAK`, `COMPUTE`, `CLEAR BREAKS`, `CLEAR COMPUTES`
- `SPOOL`
- `WHENEVER SQLERROR`, `WHENEVER OSERROR`
- `@`, `@@`, `START`
- `CONNECT`, `DISCONNECT`, `EXIT`, `QUIT`

MySQL / MariaDB 계열:

- `USE`
- `SHOW DATABASES`, `SHOW TABLES`, `SHOW COLUMNS`
- `SHOW CREATE TABLE`
- `SHOW PROCESSLIST`, `SHOW VARIABLES`, `SHOW STATUS`
- `SHOW WARNINGS`, `SHOW ERRORS`
- `DELIMITER`
- `SOURCE`

### 오브젝트 브라우저

- DB 종류에 따라 루트 카테고리를 다르게 표시
- 필터 가능한 트리 UI
- 오브젝트 새로고침
- 테이블/뷰 데이터 조회
- 구조 보기
- 인덱스 보기
- 제약조건 보기
- DDL 생성
- 패키지 루틴 표시

Oracle 루트 카테고리:

- Tables
- Views
- Procedures
- Functions
- Sequences
- Triggers
- Synonyms
- Packages

MySQL / MariaDB 루트 카테고리:

- Tables
- Views
- Procedures
- Functions
- Triggers
- Events
- Sequences는 실제로 감지될 때만 표시

### 결과 뷰

- 데이터 탭과 메시지 탭 분리
- 결과 탭별 상태 표시
- CSV 내보내기
- 선택 셀 복사
- 헤더 포함 복사
- 셀 미리보기 최대 길이 설정
- lazy fetch batch size 설정
- lazy fetch 추가 가져오기, 전체 가져오기, 취소
- Oracle 단일 테이블 결과셋의 `ROWID` 기반 staged edit
  - Insert
  - Delete
  - Save
  - Cancel
  - Set Null

결과 그리드 편집은 모든 SELECT에 열리는 기능이 아닙니다. 현재 구현은 안전하게 식별 가능한 Oracle 단일 테이블 결과셋을 전제로 하며, JOIN 결과나 `ROWID`를 안정적으로 붙일 수 없는 결과는 편집 대상으로 보지 않습니다.

### 설정, 로그, 복구

- UI/editor/result font 설정
- 결과 셀 미리보기 길이 설정
- lazy fetch batch size 설정
- 연결 풀 크기 설정
- 앱 설정 저장
- 앱 로그 뷰어
- 로그 내보내기와 비우기
- panic hook 기반 `crash.log` 기록
- 다음 실행 시 이전 크래시 리포트 표시
- 레거시 `oracle_query_tool` 설정/키링 네임스페이스 마이그레이션

## 실행

이 워크스페이스에는 여러 바이너리가 있으므로 실행 시 `--bin space_query`를 지정해야 합니다.

개발 실행:

```bash
cargo run --bin space_query
```

릴리스 실행:

```bash
cargo run --release --bin space_query
```

## 테스트

전체 테스트:

```bash
cargo test
```

빌드 확인:

```bash
cargo check
```

일부 테스트는 외부 DB나 환경 변수가 필요할 수 있습니다. Oracle/MySQL/MariaDB 실제 연결 테스트를 돌릴 때는 로컬 DB, 계정, 클라이언트 라이브러리 설정을 먼저 맞춰야 합니다.

## Oracle 클라이언트 (OCI 모드)

Thin 모드는 추가 클라이언트 없이 연결됩니다. OCI(thick) 모드에서만 Oracle Instant Client 또는 Full Client가 필요하며, 클라이언트 라이브러리를 다음 순서로 자동 탐색합니다.

1. `ORACLE_CLIENT_LIB_DIR` 환경변수가 가리키는 디렉터리
2. `ORACLE_HOME` 환경변수 (Windows: `%ORACLE_HOME%\bin`, Linux/macOS: `$ORACLE_HOME/lib`)
3. 플랫폼별 기본 위치의 `instantclient_*` 디렉터리
   - macOS: `/opt/oracle`
   - Linux: `/opt/oracle`, `/usr/local/oracle`
   - Windows: `C:\oracle`, `%ProgramFiles%\Oracle`

자동 탐색이 맞지 않으면 `ORACLE_CLIENT_LIB_DIR`를 직접 지정하세요.

```bash
export ORACLE_CLIENT_LIB_DIR=/opt/oracle/instantclient_23_3
cargo run --release --bin space_query
```

Apple Silicon에서는 앱과 클라이언트 라이브러리의 CPU 아키텍처가 같아야 합니다.

### TNS alias 연결

TNS alias 연결은 OCI 모드에서만 지원하며, alias 해석은 Oracle Net이 `tnsnames.ora`를 읽어 수행합니다. `TNS_ADMIN` 환경변수를 `tnsnames.ora`가 있는 디렉터리로 지정하세요. 지정하지 않으면 `$ORACLE_HOME/network/admin`을 사용하며, Instant Client에는 이 기본 경로가 없으므로 `TNS_ADMIN`이 사실상 필수입니다.

```bash
export TNS_ADMIN=/opt/oracle/network/admin
```

Thin 모드는 Host/Port/Service 연결만 지원하며 TNS alias는 지원하지 않습니다.

## Linux 빌드 참고

GUI 실행에는 FLTK/X11 런타임 의존성이 필요합니다. 빌드 전 해당 개발 패키지(`libxinerama`, `libxcursor`, `libxfixes`, `libxft` 등)를 설치하세요.

## 저장 위치

경로의 OS별 루트는 `dirs` crate가 결정하고, 앱 디렉터리 이름은 `space_query`입니다.

- 설정 파일: `config_dir()/space_query/config.json`
- 앱 로그: `data_dir()/space_query/app.log.json`
- 크래시 로그: `data_dir()/space_query/crash.log`
- 비밀번호: OS Keyring의 `space_query` 서비스

참고:

- 연결 정보와 최근 SQL 파일 목록은 설정 파일에 저장됩니다.
- 비밀번호는 설정 JSON에 저장하지 않습니다.
- 쿼리 히스토리는 현재 실행 중인 앱 프로세스의 메모리에서 관리됩니다.

## 라이선스와 상표

이 프로젝트는 `MIT OR Apache-2.0`으로 배포됩니다. 전체 라이선스는 `LICENSE-MIT`, `LICENSE-APACHE`를 보세요.

TNS thin 구현은 `python-oracledb`와 `go-ora`의 permissive-licensed 구현을 참고했습니다. 관련 고지는 `THIRD_PARTY_NOTICES.md`와 `crates/tns-thin/THIRD_PARTY_NOTICES.md`에 유지합니다.

Oracle, Java, MySQL, and NetSuite are registered trademarks of Oracle and/or its affiliates. Other names may be trademarks of their respective owners. 이 프로젝트는 Oracle과 제휴, 승인, 후원 관계가 아닙니다.

## 소스 구조

```text
src/
├── app.rs / main.rs   # 앱 시작/종료, 설정·크래시 로드, FLTK 초기화
├── db/                # DB 연결·세션·트랜잭션, 쿼리 실행과 스크립트 파서
├── ui/                # 메인 창, 편집기, 오브젝트 브라우저, 결과 그리드 등 FLTK UI
├── sql_*              # SQL 토큰/파서/포맷/구분 처리
└── utils/             # 설정, 자격 증명 저장, 로깅
```

- `crates/tns-thin/`: Oracle Database와 통신하는 thin TNS client crate
- `tests/`: 스레드 안전성·panic guard·회귀 가드 테스트
