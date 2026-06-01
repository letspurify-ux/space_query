# SQL Auto Formatting Depth Principles

> 최종 업데이트: 2026-04-13 (runtime formatter structural frame 단일 stack 계약을 명시하고 query/condition/block owner 복원 경로를 통합)

## 0. 이 문서의 역할

이 문서는 아래 3층을 구분해서 적는다.

- 근본 원칙: 새 구문이 추가돼도 쉽게 바뀌면 안 되는 규칙
- 구조 계약: analyzer와 formatter가 반드시 보존해야 하는 상태 모델
- 현재 taxonomy / policy: 지금 코드베이스가 채택한 keyword 집합, owner family, 렌더링 정책

정리 기준:

- "왜 항상 그래야 하는가"는 근본 원칙에 둔다.
- "어떤 상태를 들고 가야 하는가"는 구조 계약에 둔다.
- "현재는 어떤 keyword를 이렇게 분류하는가"는 taxonomy / policy에 둔다.

## 1. 근본 원칙

### 1.1 depth는 시각 indent가 아니라 구조 상태에서 결정된다

depth는 현재 시점에 활성화된 syntactic owner stack의 높이다.

- 여기서 "모든 구문이 depth +- 규칙을 따른다"는 말은 "모든 토큰마다 전용 예외를 만든다"는 뜻이 아니다. 대상은 open/close/anchor/query-head/continuation carry처럼 structural event를 만드는 syntax family이며, 구조 이벤트를 만들지 않는 토큰은 shared lexical/operator taxonomy로 처리해야 한다.
- depth는 기존 공백 수, hanging indent, 수동 정렬에서 역산하면 안 된다.
- `existing_indent`는 구조 계산의 보정 기준, soft floor, tie-breaker가 될 수 없다.
- 이전 줄 정보를 쓰더라도 반드시 이름 붙은 pending/active structural state로 승격된 값만 써야 한다.
- `직전 줄이 콤마였다`, `직전 줄이 THEN이었다`, `직전 줄이 '('였다` 같은 anonymous line-shape heuristic로 depth를 복원하면 안 된다.
- line-shape 정보는 "지금 pending state를 소비할 차례인가"를 판별하는 lexical adjacency 용도로만 허용된다.
- `fallback`이라는 이름의 무구조 보정 계층도 두면 안 된다. owner/header/query/close family가 아니면 반드시 shared lexical 또는 operator family 중 하나로 명시 분류해야 하며, "마지막 else"는 구조적 판정을 덮어쓰는 탈출구가 아니라 residual family 선택을 뜻해야 한다.

### 1.2 모든 open event는 정확히 +1이다

한 번의 opener가 구조적으로 두 단계 이상을 열면 안 된다.

- 다단계 깊이 변화처럼 보여도 실제 모델은 owner/frame push의 합성이어야 한다.
- 줄 단위 `final depth` 차이가 2 이상으로 보이는 것은 허용되지만, 그것은 여러 open/close/anchor event가 한 줄에 함께 반영된 결과여야 한다.
- `(`/`)` event는 반드시 token이 등장한 순서대로 적용해야 한다. 같은 line의 close/open을 signed net delta로 합치면 안 된다.
- 특히 `close -> open` 순서(`expr ) + (` 같은 형태)는 `saturating_sub` 이후 `+1`이 반영돼야 하므로, 이벤트를 순차 적용하지 않으면 depth가 1단계 낮게 계산될 수 있다.
- trailing owner open을 별도 처리하는 경로여도, trailing open을 제외한 나머지 same-line paren event는 여전히 token-order로 순차 적용해야 한다.

### 1.3 pure close align은 pop된 owner depth를 따른다

닫힘 정렬은 "이전 줄보다 한 단계 덜 들여쓰기"가 아니라 실제로 pop된 owner의 depth를 기준으로 한다.

- `)`는 pop된 paren owner depth에 정렬한다.
- multiline function/expression wrapper의 compact close(`SUM (...)`, `CONCAT (...)`, `COUNT (...)`처럼 opener가 같은 line에 있는 경우)도 예외가 아니다. close line은 visual 현재 depth가 아니라 opener owner depth로 돌아가야 한다.
- `END`, `END CASE`, `END IF`, `END LOOP`도 pop된 block owner depth에 정렬한다.
- query close line은 stored query close alignment를 사용한다.

### 1.4 leading close는 항상 먼저 소비한다

줄이 leading `)`로 시작하면 그 close event를 먼저 소비하고, 남은 tail을 현재 줄 구조로 다시 해석해야 한다.

- `) ORDER BY ...` 는 raw line 전체가 아니라 `ORDER BY ...`를 구조 tail로 분류한다.
- `) AND ...`, `) OR ...`, `) FOR UPDATE ...`도 같은 규칙을 따른다.
- `) FROM`, `) GROUP BY`, `) ORDER BY`, `) WINDOW ... AS (`, `) CURSOR`, `) MULTISET`, `) DENSE_RANK LAST`, `) AFTER MATCH SKIP TO`, `) UNIQUE SINGLE REFERENCE` 같은 exact bare clause/header/owner fragment도 close를 소비한 structural tail 기준으로 bare-header family를 판정해야 한다.
- mixed leading-close line은 close align과 final depth를 구분해서 해석해야 한다.
- `pending` paren frame 승격(예: standalone `(` 이후 query/non-query 판정)도 예외가 아니다. leading close로 같은 line에서 바로 닫히는 pending frame은 먼저 pop 대상에 포함하고, 살아남은 frame에 대해서만 structural tail keyword로 승격/비승격을 결정해야 한다.
- 이 정규화는 caller 습관이 아니라 shared owner/header helper의 책임이어야 한다. caller 한 곳만 `structural tail`로 전처리하고 helper 본문이 raw line을 가정하면 `) REFERENCE ... ON`, `) WINDOW ... AS`, `) OPEN ... FOR`, `) CURSOR`, `) WITHIN GROUP`, `) AFTER MATCH SKIP`, `) RETURN ALL ROWS` 같은 mixed leading-close owner/header가 phase마다 다른 depth를 만들게 된다.
- 단, close를 소비한 뒤 structural tail이 비었다고 해서 close event 자체가 사라지는 것은 아니다. nested paren을 추적하는 pending header/owner는 pure `)` line을 "빈 줄"이 아니라 "wrapper close continuation step"으로 해석해야 한다.

### 1.5 deferred structural effect는 typed pending state로 유지한다

한 줄에서 끝나지 않는 구조 효과는 "깊이 하나"로 축약하면 안 된다.

- split owner/header
- split `MERGE WHEN ... THEN` branch header
- completed owner anchor
- completed deferred-wrapper owner anchor (same-line condition owner `EXISTS`/`IN`/`ANY`/`SOME`/`ALL`, same-line retained unary modifier가 붙은 `NOT EXISTS`/`NOT IN`, direct from-item owner `LATERAL`/`TABLE`/`CROSS|OUTER APPLY`, generic expression owner `CURSOR`/`MULTISET`, PL/SQL child-query owner `CURSOR ... IS|AS` / `OPEN ... FOR`)
- deferred query head
- pure/mixed close align
- split plain `END` suffix/label carry
- standalone wrapper carry
- parenthesized expression carry

이런 상태는 필요한 의미를 잃지 않는 typed pending state로 유지해야 한다.

- 같은 line이 dedicated pending family와 generic owner/header carry에 동시에 등록되면 안 된다.
- 예를 들어 active `MERGE` branch header의 `WHEN NOT` / standalone `NOT` fragment는 merge-header pending state로만 남아야지, generic `NOT EXISTS` owner carry로 중복 승격되면 안 된다.

### 1.6 analyzer와 formatter는 같은 semantics를 공유해야 한다

`auto_format_line_contexts`와 formatter phase 2는 같은 구조 의미를 공유해야 한다.

- 한 phase만 아는 owner/frame은 허용하지 않는다.
- stable anchor, owner-relative family, continuation boundary도 같은 semantics를 따라야 한다.
- exact bare header classifier와 generic leading-prefix continuation은 같은 consumer가 아니다. `WITHIN GROUP`, `REFERENCE`, `WINDOW`, `FROM TABLE`, `LEFT JOIN TABLE`처럼 bare exact line 자체가 same-depth owner/header family인 경우는 dedicated bare-header taxonomy로 판정해야 하고, inline comment split용 generic prefix carry와 같은 함수에서 같은 depth 규칙으로 뭉개면 안 된다.
- 따라서 "inline comment가 붙은 structural prefix"에서도 exact bare owner/header line과 generic last-keyword consumer가 충돌하는 family(`JOIN`, `REFERENCE`, `WINDOW`, `FROM TABLE`, `... JOIN TABLE` 등)는 dedicated bare-header taxonomy를 먼저 참조할 수 있어야 한다. 다만 이 우선순위를 모든 exact bare line에 일괄 적용하면 `FOR UPDATE`, `RULES`, `AFTER MATCH SKIP`처럼 inline comment에서 body depth를 유지해야 하는 family까지 깨지므로, owner/header collision family로 범위를 한정해야 한다.
- inline comment용 exact bare owner/header collision family는 별도 문자열 목록으로 유지하면 안 된다. shared owner/pending classifier 결과에서 파생돼야 하며, `RIGHT/FULL/... JOIN TABLE`, `WITHIN GROUP`, `KEEP` 같은 modifier/owner variant가 새로 생겨도 같은 semantic family면 자동으로 같은 우선순위를 따라야 한다.
- 반대로 named owner line(`WINDOW w_sales AS`, `OPEN c_emp FOR`, `CURSOR c_emp IS`)은 generic header consumer가 아니다. 이런 line은 owner/pending-owner family로만 해석하고, generic leading-prefix continuation은 owner/header classifier가 모두 아니라고 명시 판정된 뒤에만 선택되는 residual lexical family여야 한다.
- 이때 "named owner line이 generic header consumer가 아니다"는 "항상 inline-comment header continuation kind를 반환하지 않는다"와 동치다. standalone wrapper/query head를 다음 line에서 same-depth로 붙이는 책임은 generic prefix carry가 아니라 pending owner anchor/state machine이 질 수 있다. 예를 들어 `REFERENCE ref ON`처럼 surviving tail이 exact owner token으로 닫히는 family와 `WINDOW w_sales AS`, `OPEN c_emp FOR`처럼 named owner anchor가 다음 `(` / child query를 직접 이어야 하는 family는 같은 named-owner 계열이어도 continuation helper의 반환값이 같을 필요가 없다.
- exact bare keyword-only header line의 continuation taxonomy도 예외가 아니다. inline comment split용 prefix classifier와 bare-line classifier가 서로 다른 hand-maintained keyword list를 가지면 안 된다.
- 특히 leading prefix continuation helper가 일부 owner-relative family를 raw literal 예외로 따로 들고 있으면 안 된다. `KEEP`의 `DENSE_RANK` / `DENSE_RANK LAST` / comment-glued `DENSE /* ... */ RANK`처럼 lexical shape가 달라도 같은 semantic family면 shared owner-relative sequence matcher로 먼저 판정해야 한다. 그 다음에야 "bare carry는 same-depth, structural prefix carry는 body depth" 같은 depth mapping 차이를 별도 단계에서 적용할 수 있다.
- 특히 exact bare owner-relative split-body-header fragment(`DENSE_RANK`, `DENSE_RANK FIRST`, `AFTER MATCH`, `AFTER MATCH SKIP TO`, `UNIQUE SINGLE REFERENCE` 등)는 trailing 마지막 keyword만 보고 generic body-depth로 분류하면 안 된다. 이런 line은 shared owner-relative sequence matcher가 "아직 같은 header chain 안인지 / 이미 body operand를 여는 complete header인지"를 먼저 판정해야 한다.
- 다만 "같은 prefix taxonomy를 쓴다"가 "inline comment split과 exact bare line이 항상 같은 continuation depth를 가진다"를 의미하지는 않는다. exact bare line이 여전히 dedicated same-depth owner/header chain fragment라면 (`WITHIN GROUP`, `DENSE_RANK LAST`, `AFTER MATCH SKIP`, `LEFT OUTER`, `REFERENCE`, `CURSOR` 등) bare carry는 same-depth여야 하고, inline comment split만 body depth를 빌릴 수 있다.
- split `FOR UPDATE`도 같은 원칙을 따른다. exact bare `FOR`는 same-depth pending header fragment이고, completed `FOR UPDATE`는 query-base+1 body-carry header다. 두 phase를 같은 literal `FOR` rule로 뭉개면 `FOR -- ...` / `UPDATE ...` 와 `FOR UPDATE -- ...` / `SKIP LOCKED`가 서로 다른 semantic state를 잃는다.
- overloaded keyword prefix도 exact structural sequence로 끊어야 한다. dedicated family가 `FOR UPDATE`라면 exact structural token sequence `FOR UPDATE`와 exact bare split `FOR`만 그 family다. `FOR ORDINALITY`, `FOR rec IN`, table-function/item syntax처럼 같은 `FOR` prefix를 쓰는 다른 구문은 dedicated `FOR UPDATE` rule에 들어가면 안 된다.
- stable query-head taxonomy와 bare-header continuation taxonomy도 서로 독립이면 안 된다. single-keyword query head가 dedicated owner family가 아니라면 exact bare line과 inline-comment split line이 같은 shared continuation kind를 가져야 한다. 예를 들어 `CALL`은 query-head boundary helper에는 있지만 bare-header carry에서 빠지면 `CALL -- ...` / `pkg.do_work (...)`와 bare `CALL` / `pkg.do_work (...)`가 모두 구조 carry를 잃어 root rule 1.6과 1.7을 동시에 깨게 된다.
- condition/operator RHS continuation taxonomy도 shared semantic family여야 한다. trailing RHS operator, mixed leading-close expression continuation, trailing inline-comment continuation은 서로 다른 ad-hoc keyword table을 가지면 안 되며, `MEMBER OF`, `SUBMULTISET OF`, `LIKEC/LIKE2/LIKE4`, `ESCAPE`, `:=`, `=>` 같은 family를 한 phase만 알게 두면 안 된다.
- trailing inline-comment continuation은 "operator RHS family" 하나로 축약하면 안 된다. structural header carry가 필요한 family(`JOIN`, `ON`, `USING`, `WINDOW`, `SELECT`, `SET` 등)는 shared bare/header taxonomy에서 파생된 header family로, `AND`, `LIKE4`, `MEMBER OF`, `:=` 같은 RHS family는 shared operator taxonomy로 판정해야 한다.
- 같은 trailing token이 lexical하게는 operator RHS처럼 보여도 structural owner family와 충돌하면 structural owner가 우선한다. 예를 들어 exact bare / completed deferred-wrapper owner line인 `EXISTS`, `IN`, `ANY/SOME/ALL`, `NOT EXISTS`, `NOT IN` 뒤 inline comment는 generic operator `+1` residual path로 재분류하면 안 되고, shared owner classifier가 산출한 same-depth owner carry를 먼저 유지해야 한다.
- exact bare line이 "다음 code line에서 wrapper/query head를 받을 completed owner anchor"인 경우도 예외가 아니다. `EXISTS`, `IN`, `ANY/SOME/ALL`, same-line `NOT EXISTS`/`NOT IN`, `LATERAL`, `TABLE`, `CROSS/OUTER APPLY`, `CURSOR`, `MULTISET`처럼 standalone `(` 또는 child query가 뒤로 밀린 owner line은 generic `FROM`/`WHERE` body header처럼 다시 +1 하지 말고 same-depth owner family로 유지해야 한다.
- same token이 carry를 열고/닫거나 frame reset을 일으키는 경우도 예외가 아니다. semicolon/comma/standalone `(` 같은 punctuation-driven state transition은 analyzer와 formatter가 같은 trailing/standalone structural helper를 공유해야 한다.
- `WITH` sibling CTE definition header 판정도 예외가 아니다. `cte_name AS (`와 `cte_name (col1, ...) AS (`는 local analyzer heuristic가 아니라 shared structural helper로 분류해야 다음 CTE/main query가 continuation state를 잘못 상속하지 않는다.
- 특정 구문이 애매하면 "같은 semantic decision을 양쪽에서 재현한다"가 원칙이고, helper를 하나로 합칠지 전용 판별식을 둘지는 구현 전략이다.
- shared classifier가 반환한 continuation kind를 실제 numeric depth로 바꾸는 단계도 예외가 아니다. `SameDepth` / `OneDeeperThanQueryBase` / `OneDeeperThanCurrentLine` 해석을 analyzer, formatter, operator-RHS carry가 각자 hand-written match arm으로 복제하면 새 family 추가 시 phase drift가 생기므로 공용 resolver를 사용해야 한다.
- 이 resolver는 최소한 `same-depth anchor`, `current-line anchor`, `query-base anchor`를 구분할 수 있어야 한다. comment split renderer처럼 `SameDepth`는 owner/header line에 snap 해야 하지만 `OneDeeperThanCurrentLine`은 이미 증가된 현재 줄 depth에서 한 단계 더 가야 하는 경로가 있기 때문이다.
- 여기서 `query-base anchor`는 항상 active query frame의 저장 depth와 동일한 값일 필요는 없다. renderer처럼 local formatting context만 가진 caller는 semantic query base를 나타내는 synthetic anchor를 전달할 수 있어야 하며, resolver는 그 차이를 이름과 계약 수준에서 드러내야 한다.
- renderer/helper wrapper도 이 세 anchor를 하나의 `base indent`로 뭉개면 안 된다. exact bare owner/header family(`REFERENCE`, `WITHIN GROUP`, deferred-wrapper owner 등)에서 `SameDepth`는 현재 owner/header line depth를 써야 하고, clause family(`WHERE`, `FOR UPDATE` 등)의 `query-base+1`만 semantic query base anchor를 써야 한다.
- raw previous/last-word 기반 continuation helper는 독립 구조가 아니라 residual lexical classifier일 뿐이다. exact bare owner/header/pending classifier가 semantic family를 이미 판정한 line(`AFTER MATCH SKIP -- ...`, `DENSE_RANK LAST -- ...`, `INNER JOIN -- ...` 등)에서는 이 residual classifier가 depth를 다시 결정하면 안 된다.
- non-subquery 일반 괄호 내부의 function-local option clause는 구조 clause가 아니다. `JSON_VALUE(... RETURNING VARCHAR2 (...))`, `XMLQUERY(... RETURNING CONTENT)`, `JSON_QUERY(... WITH WRAPPER)`, `JSON_EXISTS(... TRUE ON ERROR / FALSE ON EMPTY)` 같은 line은 function-local option으로만 해석해야 하며, analyzer/query-role/structural-boundary/header-carry helper 중 한 phase라도 이를 top-level clause로 승격하면 다음 sibling list item까지 잘못된 carry가 누수된다. 단, `FROM`은 scalar subquery 내부에서 실제 구조 clause가 될 수 있으므로 blanket suppress 대상이 아니다.

### 1.7 formatter output은 canonical하고 idempotent해야 한다

자동 포맷팅의 목표는 "지금 보이는 공백을 그럴듯하게 손본다"가 아니라 같은 구조 의미를 같은 canonical layout으로 수렴시키는 것이다.

- 같은 structural state를 가진 line은 입력 공백/주석 gap/history와 무관하게 같은 canonical depth로 수렴해야 한다.
- 한번 canonical form으로 정규화된 결과에 다시 formatter를 적용해도 depth/line break/anchor 선택이 바뀌면 안 된다.
- 따라서 formatter는 입력의 기존 indent를 "유지할지 말지" 결정하는 도구가 아니라, shared structural semantics를 canonical layout으로 투영하는 renderer여야 한다.
- idempotence는 별도 미적 요구가 아니라 근본 검증 규칙이다. 새 family를 추가할 때는 "semantic family 판정", "typed pending state 유지", "anchor resolver 적용"뿐 아니라 "두 번 돌려도 같은 결과인지"까지 확인해야 한다.
- mixed leading-close, owner-relative header chain, comment-glued split owner처럼 phase drift가 잘 생기는 family는 canonical form이 한 번에 고정되어야 한다. 첫 번째 포맷에서 임시 보정하고 두 번째 포맷에서 다시 depth가 바뀌는 구조는 허용하지 않는다.
- sibling list도 같은 원칙을 따른다. 하나의 list item 안에서 general paren / owner-relative body / function option line 때문에 임시로 더 깊어질 수는 있지만, trailing comma가 item을 닫은 순간 다음 sibling은 항상 stable `list body depth`로 복귀해야 한다. 이전 item의 function-local `RETURNING` carry나 visual hanging indent가 다음 sibling의 anchor가 되면 canonical form이 아니다.
- 특히 compact argument list의 `CASE ... END,` 뒤 sibling operand는 `WHEN/ELSE` branch literal depth가 아니라 parent frame의 argument body depth로 즉시 복귀해야 한다. `END,` 직후 `CONCAT(...)`/`JSON_OBJECT(...)` 같은 sibling가 branch depth를 상속하면 canonical depth 규칙(1.7)을 위반한다.

### 1.8 comment와 quoted literal은 구조 이벤트가 아니다

주석/문자열/quoted identifier 안의 토큰은 depth event를 만들거나 지우면 안 된다.

- trailing inline comment나 inline block comment가 split owner/open/close recognition을 끊으면 안 된다.
- inline comment를 넣었다고 continuation semantic family가 바뀌면 안 된다. `AND a =` 와 `AND a = /* gap */`, `v_total :=` 와 `v_total := /* gap */`처럼 comment 유무만 다른 line은 같은 shared continuation classifier를 써야 한다.
- leading block comment가 붙은 code line도 comment-only line으로 취급하면 안 된다. `/* note */ ON ...`, `/* note */ ORDER BY ...`, `/* note */ BEGIN ...` 같은 line은 첫 meaningful structural token부터 다시 분류해야 한다.
- line head prefix 판정도 예외가 아니다. `CREATE /* gap */ MATERIALIZED /* gap */ VIEW ... AS`, `OPEN c /* gap */ FOR`, `CURSOR c /* gap */ IS`, `WHEN /* gap */ NOT /* gap */ MATCHED THEN`, `MATCH /* gap */ RECOGNIZE` 같은 multi-keyword owner/header는 raw prefix string이나 `contains()`가 아니라 comment-stripped meaningful identifier sequence로 판정해야 한다.
- control-branch owner/header 판정도 예외가 아니다. exact `ELSE`, exact `EXCEPTION`, `ELSIF ... THEN`, `ELSEIF ... THEN`, `CASE` 같은 PL/SQL branch header는 `ELSE/* gap */`, `EXCEPTION/* gap */`, `ELSIF/* gap */ cond THEN` 형태여도 raw `trim()`/`starts_with()`가 아니라 comment-stripped structural token sequence로 판정해야 한다.
- control-body owner와 control-condition header는 별도 family다. bare `IF`/`ELSIF`/`ELSEIF` condition line은 `THEN`이 structural token sequence에 나타나기 전까지 body owner가 아니며, exact bare `IF (`/`ELSIF (`/`ELSEIF (`는 condition-header lookahead로만 다뤄야 한다.
- bare control-condition header lookahead도 예외가 아니다. `IF /* gap */ (`, `ELSIF /* gap */ (`, `ELSEIF /* gap */ (`, `WHILE /* gap */ (`, `WHEN /* gap */ (` 같은 exact bare header는 다음 pure `)` / `AND` / `OR` line이 retained condition state를 정확히 복원할 수 있도록 shared structural helper로 판정해야 한다.
- underscore를 가진 composite keyword(`MATCH_RECOGNIZE`, `DENSE_RANK` 등)도 예외가 아니다. 구현 내부 표현이 단일 identifier이든, comment/whitespace 때문에 여러 identifier segment로 보이든 structural classifier는 같은 keyword sequence로 취급해야 한다.
- 이 원칙은 leading/trailing header continuation classifier와 owner-relative split body-header matcher에도 동일하게 적용된다. 특정 phase만 raw word pair 비교를 쓰면 같은 composite keyword가 줄 위치에 따라 다른 depth를 만들게 된다.
- same-line classifier뿐 아니라 next/previous-line lookahead도 예외가 아니다. split owner lookahead, blank-line suppression, case-close alignment, control-condition close 판정처럼 "인접 line의 첫 구조 토큰"을 보는 로직도 raw `trim()`/`starts_with()`가 아니라 comment-stripped structural tail 기준으로 판정해야 한다.
- `END /* gap */ IF`, `END -- gap` 다음 suffix line, `) /* gap */ ORDER BY` 같은 형태도 주석을 제거한 structural token sequence로 판정해야 한다.
- line tail/suffix 판정도 예외가 아니다. `GROUP /* gap */ BY -- ...`, `FOR /* gap */ UPDATE -- ...`, `IF ... /* gap */ THEN`처럼 split header나 trailing terminator를 판정할 때도 raw whitespace split이 아니라 comment-stripped meaningful identifier sequence를 사용해야 한다.
- statement terminator 판정도 예외가 아니다. `OPEN c_emp; -- done`, `CURSOR c_emp IS; /* impossible but lexical */`, `END; -- block close` 같은 line은 raw `trim_end().ends_with(';')`가 아니라 inline comment를 제거한 뒤의 마지막 meaningful token으로 닫힘 여부를 판정해야 한다.
- leading close 선소비도 동일하다. line 시작 시 parser lex state가 이미 multiline literal/quoted identifier(`'...'`, `"..."`, `` `...` ``, `q'...'`, `$$...$$`) 내부라면 line head의 `)`는 구조 close가 아니라 literal payload이므로 leading-close event로 집계하면 안 된다.
- exact bare header / standalone wrapper 판정도 예외가 아니다. `FROM /* gap */`, `WHERE /* gap */`, `ON /* gap */`, `( -- wrapper` 같은 line은 raw `trim()` / `==` 비교가 아니라 shared structural token / wrapper helper로 판정해야 한다.
- 구조 helper는 필요하면 "원문 문자열"이 아니라 "comment를 제거한 meaningful token sequence"를 기준으로 동작해야 한다.
- line-local helper뿐 아니라 statement-level 구조 플래그도 동일하다. `PACKAGE BODY` 여부, `APPLY` family 활성 같은 전역 판정은 raw statement `contains(...)`가 아니라 meaningful token sequence에서 도출해야 한다. 문자열/주석 안 텍스트가 구조 플래그를 켜면 1.8 위반이다.

## 2. 구조 계약

### 2.1 owner line과 anchor line은 구분한다

모든 depth 재고정 line이 owner push/pop을 의미하는 것은 아니다.

- owner line: 실제로 owner/frame을 열거나 닫는 line
- anchor line: stack 높이는 바꾸지 않지만 현재 line depth나 다음 body/query depth를 다시 고정하는 line

즉 formatter는 "owner가 열렸는가"와 "정렬 anchor가 바뀌었는가"를 분리해서 다뤄야 한다.

### 2.2 구조 계산에서 구분해야 하는 값

| 이름 | 의미 |
|---|---|
| `owner depth` | owner header line 자신의 구조 depth |
| `body depth` | owner가 여는 본문 depth. 항상 `owner depth + 1` |
| `list body depth` | sibling list가 이어지는 body depth |
| `close align depth` | leading close를 먼저 소비했을 때 참조하는 구조 정렬 depth |
| `final depth` | leading close 소비와 continuation/body/header 해석까지 끝난 현재 줄의 최종 structural depth |
| `render indent` | 렌더링 공백 수 |

현재 구현 대응:

- `parser_depth`: lexical leading close를 먼저 소비한 뒤의 기본 structural depth
- `auto_depth`: analyzer가 계산한 현재 code line의 structural depth
- `query_base_depth`: 현재 query frame의 base depth
- `next_query_head_depth`: 현재 owner가 여는 다음 child query head depth
- `final_depth`: formatter phase 2가 정규화를 마친 최종 structural depth

### 2.3 owner align depth와 owner base depth는 다르다

같은 숫자로 시작할 수는 있어도 의미가 다르다.

- wrapper / split-header / close alignment에는 `owner align depth`를 쓴다.
- child query head 계산에는 `owner base depth`를 쓴다.
- 두 값을 섞으면 split owner와 child query indentation이 쉽게 뒤틀린다.

### 2.4 completed owner anchor는 별도 상태로 유지한다

split owner/header가 현재 줄에서 완성됐더라도, child body/query/open wrapper가 다음 code line에서 시작하면 owner를 즉시 잊으면 안 된다.

- completed owner anchor는 완성된 owner line의 `owner depth`를 유지하는 pending state다.
- exact bare deferred-wrapper owner line(`EXISTS`, `IN`, `ANY/SOME/ALL`, same-line `NOT EXISTS`/`NOT IN`, `LATERAL`, `TABLE`, `CROSS/OUTER APPLY`, `CURSOR`, `MULTISET`)와 completed PL/SQL child-query owner line(`CURSOR ... IS|AS`, `OPEN ... FOR`)도 동일한 completed owner anchor family다.
- 단, completed owner anchor가 다음 줄에 전달하는 효과는 단일 숫자 하나가 아니다. standalone `(` / leading `)` / header completion fragment처럼 owner line에 정확히 snap 해야 하는 소비와, ordinary child body line처럼 `owner depth + 1`을 floor로만 요구하는 소비를 typed state로 구분해야 한다.
- child query용 `owner base depth` 또는 `next_query_head_depth`를 추가로 들고 갈 수 있다.
- 하지만 wrapper line 정렬 기준은 항상 `owner depth`다.
- comment-only / blank / comma-only line은 anchor를 소비하지 않는다.
- standalone `(`, leading `)` wrapper line도 anchor를 먼저 소비하지 않는다.

### 2.5 continuation carry는 explicit state로만 이어진다

owner를 열지도 닫지도 않는 line은 활성 stack과 explicit continuation state에 따라 해석한다.

- split owner/header chain은 최초 owner line의 구조 depth를 보존한다.
- split PL/SQL child-query header도 header 완성 전까지 owner depth를 유지한다.
- split `MERGE WHEN ... THEN` header도 `WHEN`/`WHEN NOT`/`MATCHED` fragment와 standalone `WHEN MATCHED`/`WHEN NOT MATCHED` header line은 owner depth를 유지하고, 그 다음 header condition/standalone `THEN` consuming line만 `owner depth + 1`을 사용한다.
- split header condition 안에 nested child query/owner가 들어오면 retained header state는 즉시 해제되지 않고 suspend 된다. nested child가 닫힌 뒤 merge-header를 다시 소비하는 line(`THEN`, `) THEN`, 이어지는 condition line)에서 resume 되어야 한다.
- split plain `END` suffix/label carry도 qualifier/label token이 comment에 가려져도 owner depth를 잃지 않는다.
- continuation state는 blank/comment-only/comma-only line에서 소비되지 않는다.
- continuation state는 첫 consuming code line 또는 새 owner/clause/query boundary에서만 소비되거나 해제된다.

### 2.6 mixed leading-close line은 close align과 final depth를 분리한다

- close를 소비한 뒤 tail이 비었거나 구두점만 남는 close-only line에서는 `final depth == close align depth`로 볼 수 있다.
- `) AS alias,`, `) alias,`, `) "alias",`처럼 close 이후 non-punctuation tail이 남는 line은 close-only line이 아니므로, close align을 그대로 투영하지 않고 frame stack/token-order를 다시 적용한 depth를 `final depth`로 사용해야 한다.
- delimiter가 없는 terminal alias tail(`) window`, `) AS window`)도 예외가 아니다. lexical helper 하나로 bare `WINDOW` clause와 구분하려 하지 말고, active query frame + 다음 code line boundary를 함께 봐서 "현재 select-list item의 닫힘 alias인지 / 다음 structural header chain의 시작인지"를 판정해야 한다.
- mixed leading-close line에서는 close를 먼저 소비한 뒤 continuation/body/header 규칙으로 다시 해석한 결과가 `final depth`다.
- 실제 렌더링은 token-level 2단 정렬이 아니라 line-level canonical depth를 사용한다.

### 2.7 runtime formatter의 LIFO structural state는 단일 frame stack으로 유지한다

- `formatter.rs`의 실제 auto-format runtime은 구조적 LIFO 상태를 하나의 `Vec<FormatFrame>`로만 관리한다.
- `Paren`, `Block`, `ConditionOwner`, `BetweenPending`, `TriggerHeader`, `PlsqlContext`, `CompoundTrigger`, `WithCte`, `OpenCursor` 같은 frame family는 모두 같은 stack에 들어가며, push/pop/scope sync도 이 stack 하나를 통해 수행한다.
- query-like paren의 restore state, query base depth, wrapped-owner metadata는 별도 벡터가 아니라 해당 `Paren` frame 안에 저장되어야 한다.
- block kind와 block owner depth도 분리된 pair stack이 아니라 하나의 `Block` frame으로 유지해야 하고, `CASE`/`EXCEPTION` branch 시작 여부와 MySQL handler `BEGIN` metadata도 그 frame 안에서 복원되어야 한다.
- `JOIN/WHERE/CASE/control-condition` owner와 `BETWEEN`, trigger/plsql/compound-trigger/with-cte scoped state는 dedicated frame으로 유지하되, 만료 판정은 별도 배열 truncate가 아니라 현재 `(paren_depth, block_depth)`에 대한 stack scan/sync로만 처리해야 한다.
- statement boundary cleanup도 별도 side-vector reset이 아니라 stack policy여야 한다. statement-local frame(`Paren`, `ConditionOwner`, `BetweenPending`, `OpenCursor` 등)만 tail-pop으로 제거하고, 살아 있는 `Block`/`PlsqlContext`/`CompoundTrigger`/`WithCte`는 유지해야 한다.
- scope unwind와 clause-boundary 정리는 중간 `remove(idx)`/`retain(...)`이 아니라 tail-pop 기반 stack discipline으로만 수행해야 한다. 같은 원칙은 `WithCte` frame 내부의 nested body frame에도 적용된다.
- 따라서 active query base, parent sibling indent, active wrapped owner kind, CASE/control-header owner depth 같은 파생값도 "각각의 side stack"을 읽어 합성하지 않고, 항상 단일 frame stack을 뒤에서부터 스캔해서 계산해야 한다.
- non-LIFO 성격의 bool/phase flag는 별도 상태로 남을 수 있지만, 구조 depth와 owner 복원에 관여하는 값은 단일 frame stack 밖에 복제되면 안 된다.

## 3. 현재 taxonomy / policy

이 절은 현재 코드베이스의 taxonomy와 policy를 적는다.
새 구문이 늘거나 분류가 바뀌면 이 절은 바뀔 수 있지만, 1절과 2절의 원칙은 가능한 유지해야 한다.

### 3.1 현재 owner family

현재 depth 모델이 명시적으로 다루는 owner family:

- 일반 괄호 표현식
- 서브쿼리 소유 괄호
- condition-owned child-query owner (`EXISTS`, `IN`, `ANY/SOME/ALL`, split `NOT ...`)
- direct from-item child-query owner (`LATERAL`, `TABLE`, `APPLY` child-query branch)
- generic expression child-query owner (`CURSOR`, `MULTISET`)
- `BEGIN … END`, `CASE … END`, `IF … END IF` 블록
- MySQL/MariaDB compound routine block owner (`BEGIN`, labeled `LOOP`, `REPEAT`, `WHILE … DO`)
- `OVER (…)`, `WITHIN GROUP (…)`, `WINDOW (…)`, `MATCH_RECOGNIZE (…)`, `PIVOT (…)`, `UNPIVOT (…)`, `MODEL (…)`, `JSON_TABLE ... NESTED/COLUMNS (...)` 같은 multiline clause owner
- `CURSOR ... IS`, `OPEN ... FOR`, control-body query owner 같은 PL/SQL child-query owner
- `THEN`, `ELSE`, `EXCEPTION` body owner
- `MERGE WHEN ... THEN`, `INSERT ALL/FIRST`, `FORALL` 같은 DML/PLSQL body owner
- `CREATE TRIGGER` header body owner
- `CREATE TABLE ... AS`, `CREATE [MATERIALIZED] VIEW ... AS` 같은 DDL header body owner

주의:

- bare `IF`/`ELSIF`/`ELSEIF` condition header는 owner family가 아니라 condition-header / wrapper family다. body owner는 `THEN`이 완료된 시점부터 열린다.
- `CREATE TRIGGER` header body owner도 "header body line"과 "body opener"를 섞으면 안 된다. `BEFORE/AFTER`, `ON`, `REFERENCING`, `FOR EACH ROW`, `WHEN`은 owner depth + 1이지만, 그 다음 `DECLARE`/`BEGIN`은 retained trigger-header state를 종료하고 owner depth로 복귀해야 한다.
- split `CREATE [MATERIALIZED] VIEW|TABLE ... AS` header chain은 trigger header body와 다르다. `BUILD DEFERRED`, `REFRESH FAST`, `ON DEMAND`, `ENABLE QUERY REWRITE`, storage/property option 같은 `AS` 이전 fragment는 최초 `CREATE ...` owner depth를 유지하고, trailing `AS`가 완료된 뒤의 query head (`WITH`/`SELECT`/`VALUES`)만 owner depth + 1에서 시작한다.
- MySQL/MariaDB compound routine block도 block owner family다. `BEGIN`, labeled `LOOP`, `REPEAT`, `WHILE … DO` header는 owner depth를 열고 본문은 항상 `owner depth + 1`을 사용해야 하며, `END LOOP` / `END REPEAT` / `END WHILE`은 opener owner depth로 정렬해야 한다.
- labeled routine block(`read_loop: LOOP`, `main_block: BEGIN`, `nested_block: BEGIN`)은 label 토큰이 추가되더라도 owner family가 바뀌지 않는다. label은 opener depth를 보존하는 장식이며, body depth나 close alignment를 별도 visual heuristic로 재계산하면 안 된다.
- routine `CASE`는 SQL expression `CASE`와 semantic family가 다르다. MySQL/MariaDB compound block 안의 statement `CASE`는 PL/SQL `CASE` branch owner와 같은 depth 규칙을 따라 `WHEN`/`ELSE`는 `CASE`보다 한 단계 deeper, branch statement는 다시 한 단계 deeper여야 한다.
- MySQL/MariaDB의 function-like syntax는 callee/type token과 `(` 사이 공백을 새로 삽입하면 안 된다. stored routine declaration header(`CREATE FUNCTION fn_x(...)`, `CALL proc(...)`), built-in/UDF call(`JSON_EXTRACT(...)`, `ROW_NUMBER()`, `CAST(...)`), type/precision spec(`VARCHAR(255)`, `DECIMAL(10,2)`, `CHAR(3)`), `ON DUPLICATE KEY UPDATE` 안의 legacy `VALUES(...)` 참조는 모두 tight paren을 유지해야 한다. 반대로 top-level `VALUES (` clause, `IF (`, `EXISTS (`, `OVER (`처럼 clause/control owner를 여는 structural keyword는 same-family가 아니므로 기존 spacing 규칙을 유지한다.
- MySQL/MariaDB 렌더링에서는 토큰이 우연히 keyword와 같은 철자를 가지더라도 실제 identifier/operand 문맥이면 원문 casing을 유지해야 한다. 특히 `CREATE TABLE` generated expression, function argument, `ON DUPLICATE KEY UPDATE` RHS, 일반 `SELECT`/`WHERE` 식 위치, `AS rank` / `FROM metrics profile` 같은 alias 위치에서 `profile`, `rank`, `window` 같은 이름을 formatter가 keyword 대문자화로 바꾸면 semantic drift가 발생하므로 허용하지 않는다.

### 3.2 current structural continuation boundary taxonomy

현재 continuation/operator RHS/header carry를 끊는 boundary는 다음 taxonomy를 사용한다.

- stable clause/query anchor: `SELECT`, `WITH`, `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `CALL`, `VALUES`, `TABLE`, `FROM`, `WHERE`, `GROUP`, `HAVING`, `ORDER`, `SET`, `INTO`, `USING`, `CONNECT`, `START`, `MODEL`, `WINDOW`, `MATCH_RECOGNIZE`, `PIVOT`, `UNPIVOT`, `SEARCH`, `CYCLE`, `RETURNING`, `OFFSET`, `FETCH`, `LIMIT`, `QUALIFY`
- join boundary: `JOIN`, `APPLY`, `LEFT/RIGHT/FULL/CROSS/NATURAL/OUTER ... JOIN|APPLY`
- join condition boundary: `ON`, `USING`
- dedicated clause boundary: `FOR UPDATE`
- owner boundary: shared `sql_text` helper가 인식하는 query owner / multiline owner / PL/SQL child-query owner
- merge branch header boundary: `WHEN`, `WHEN NOT`, `WHEN MATCHED`, `WHEN NOT MATCHED`
- standalone `(` wrapper line

주의:

- mixed leading-close line은 raw line이 아니라 close를 소비한 structural tail로 위 taxonomy에 대입한다.
- `FOR UPDATE`처럼 다른 keyword와 충돌 가능한 구문은 현재 policy상 dedicated handling을 둔다. exact bare split `FOR` fragment도 같은 dedicated family에서 해석해야 한다.
- 이때 dedicated `FOR UPDATE` family는 exact structural token sequence `FOR UPDATE`일 때만 성립한다. `FOR ORDINALITY` 같은 다른 `FOR ...` family를 prefix match로 섞으면 안 된다.
- split `MERGE` branch header는 generic condition continuation이 아니라 dedicated pending merge-branch-header state로 처리한다.
- incomplete split `MERGE` fragment(`WHEN`, `WHEN NOT`)도 shared structural boundary helper가 먼저 끊어줘야 한다. 그래야 generic continuation carry와 dedicated merge-header state가 충돌하지 않는다.
- active `WITH` frame 안의 sibling CTE definition header(`cte_name AS (` 뿐 아니라 `cte_name (col1, ...) AS (` 형태 포함)는 generic list/item continuation이 아니라 `WITH` owner depth의 stable base line이다. column list identifier 때문에 trailing `AS`를 놓치면 다음 CTE/main query가 잘못 continuation depth를 상속한다.

### 3.3 current bare header continuation taxonomy

현재 bare header continuation depth 분류:

- bare-header carry는 comment를 제거한 structural tail이 "exact keyword-only line"일 때만 켠다. `SELECT DISTINCT`는 bare header지만 `SELECT DISTINCT empno`는 bare header가 아니다.
- same-depth header: `WITH` 같은 owner/header chain 조각, terminal `JOIN/APPLY` 전의 incomplete modifier fragment (`LEFT OUTER`, `CROSS`, `OUTER`)
- same-depth merge-header header line: retained `WHEN`, `WHEN NOT`, pending state가 넘겨주는 split `MATCHED`, standalone `WHEN MATCHED`, standalone `WHEN NOT MATCHED`
- same-depth deferred-wrapper owner line: exact bare condition-owner line(`EXISTS`, `IN`, `ANY/SOME/ALL`, same-line `NOT EXISTS`/`NOT IN`), exact bare direct from-item owner(`LATERAL`, `TABLE`, `CROSS/OUTER APPLY`, `FROM/USING/... JOIN TABLE` variants), exact bare generic expression owner(`CURSOR`, `MULTISET`)
- same-depth exact bare multiline/query owner header line: `WITHIN GROUP`, `KEEP`, `MATCH_RECOGNIZE`, `PIVOT`, `UNPIVOT`, `REFERENCE`, `JOIN TABLE`/`USING TABLE` 계열처럼 generic last-keyword consumer와 충돌할 수 있는 owner/pending-owner family
- current-line+1 dedicated clause-list header: exact bare `WINDOW` when it opens a named window definition list; 이 line 자체는 dedicated `WINDOW` family로 분류하되, 다음 named window sibling은 generic same-depth가 아니라 clause body depth를 받는다
- same-depth dedicated pending fragment: exact bare split `FOR` when it is still waiting for `UPDATE`
- query-base+1 header: `FROM`, `WHERE`, `HAVING`, `USING`, `INTO`, `ON`, `UNION/INTERSECT/MINUS/EXCEPT`, `QUALIFY`, `SEARCH`, `CYCLE`, `FOR UPDATE`
- current-line+1 merge-header condition: retained merge-header state가 소비하는 `AND`/`OR` condition line, standalone `THEN`, mixed close tail `) THEN`
- current-line+1 generic header: `SELECT`, `SELECT DISTINCT/UNIQUE/ALL`, `CALL`, `VALUES`, `SET`, `RETURNING`, `OFFSET/FETCH/LIMIT`, plain exact bare `JOIN`, `GROUP BY`, `ORDER BY`, `PARTITION BY`, `START WITH`, `CONNECT BY`
- query-base+1 exact bare modifier-completed join header: `LEFT/RIGHT/FULL/INNER/CROSS/NATURAL ... JOIN`
- owner-relative body header family: `MEASURES`, `REFERENCE`, `SUBSET`, `PATTERN`, `DEFINE`, `RULES`, `COLUMNS`, `KEEP`
- owner-relative split-header state machine: active multiline owner 안에서는 `WITHIN -> GROUP`, `DENSE_RANK -> FIRST/LAST -> ORDER -> BY`, `AFTER -> MATCH SKIP -> TO NEXT ROW`, `ROWS -> BETWEEN ...`, `RETURN -> UPDATED/ALL -> ROWS`, `RULES -> AUTOMATIC/SEQUENTIAL -> ORDER` 같은 multi-step sequence를 dedicated state로 추적한다. 이 family는 bare-header carry가 있더라도 final canonical depth를 active owner body depth 기준으로 다시 snap 할 수 있다.
- inline comment split도 위 state machine의 예외가 아니다. exact bare split-body-header가 sequence 중간 단계인지, freeform suffix를 더 받아야 하는지, 이미 operand/body를 여는 complete header인지 판정하는 책임은 generic last-keyword table이 아니라 shared sequence matcher에 있다.

주의:

- exact bare deferred-wrapper owner line은 same-depth family다. `FROM`/`WHERE`/`JOIN`처럼 다음 operand/item body를 여는 generic clause header와 섞으면 wrapper line depth가 불안정해진다.
- 단, low-level split-owner lookahead helper는 semantic family 전체와 1:1이 아닐 수 있다. 예를 들어 exact `CROSS/OUTER APPLY` owner line은 same-depth deferred-wrapper owner family에는 속하지만, 구현에서는 completed owner anchor path를 통해 처리하고 split `LATERAL`/`TABLE` lookahead helper에 억지로 넣지 않는다.
- 이 family는 literal 마지막 token이 아니라 owner family로 분류한다. 따라서 `TABLE`은 generic stable anchor taxonomy에도 등장하지만, exact bare `TABLE` owner line에서는 deferred-wrapper owner family가 우선한다. 같은 이유로 same-line `NOT EXISTS`/`NOT IN`도 generic `NOT` fragment가 아니라 completed owner anchor로 본다.
- `... APPLY` terminal line은 같은 `JOIN/APPLY` modifier family라도 generic bare header가 아니라 completed deferred-wrapper owner anchor다. 반대로 terminal `APPLY` 전의 fragment (`CROSS`, `OUTER`)는 pending header chain으로 same-depth를 유지한다.
- `... JOIN` terminal line도 동일하게 semantic family를 나눠야 한다. plain exact bare `JOIN`은 current-line+1 generic header지만, modifier-completed exact bare `LEFT/RIGHT/FULL/INNER/CROSS/NATURAL ... JOIN`은 query-base+1 clause header다. 둘을 단순히 "마지막 token이 JOIN"인 literal rule로 뭉개면 mixed leading-close / comment-gap에서 wrong anchor를 타게 된다.
- `/* gap */ LATERAL`, `/* gap */ TABLE` 같은 comment-glued exact bare direct from-item owner도 동일한 same-depth deferred-wrapper family다. raw `starts_with("LATERAL"|"TABLE")`로 판정하면 shared structural tail 규칙을 깨고 FROM-item body depth로 잘못 강등된다.
- inline comment split에서 same-depth exact bare owner/header line을 먼저 고르는 기준도 literal phrase table이 아니라 shared owner/pending classifier다. `RIGHT/FULL/... JOIN TABLE`, `WITHIN GROUP`, `KEEP`처럼 modifier/owner variant를 따로 누락시키면 근본 원칙 1.6을 깨게 된다.
- `WINDOW`, `MATCH_RECOGNIZE`, `PIVOT`, `UNPIVOT`, `MODEL`은 generic bare-header continuation consumer가 아니라 dedicated owner-relative / subclause family로 관리한다.
- `MEASURES`, `REFERENCE`, `SUBSET`, `PATTERN`, `DEFINE`, `RULES`, `COLUMNS`, `KEEP`은 generic query-base carry가 아니라 active owner frame의 body depth에 먼저 snap 되어야 한다.
- split multiline owner/modifier의 exact bare keyword-only line도 generic carry와 dedicated owner-relative state를 같은 shared prefix taxonomy 위에서 해석해야 한다. 단, 최종 `final depth`는 active owner state가 canonicalize한다.
- split `MERGE` header의 standalone `THEN`은 generic bare-header consumer가 아니라 retained merge-branch-header condition depth를 그대로 사용한다.
- bare `WINDOW`는 `WINDOW w_name AS (...)` named owner의 일부가 아니라, named window definition list를 여는 clause header다. 따라서 multiline `WINDOW` clause에서 bare `WINDOW` 다음 `w_name AS (` sibling header는 `WINDOW` line과 same-depth가 아니라 clause body depth(`current-line + 1`)를 받아야 한다.
- 이때 `w_name AS (` / `w_running AS (` / `w_global AS (` 같은 named window definition sibling은 모두 같은 `WINDOW` clause body depth를 공유하고, 각 definition 내부의 `PARTITION BY` / `ORDER BY` / `ROWS|RANGE|GROUPS|EXCLUDE`는 그보다 다시 한 단계 deeper여야 한다.
- `),` 뒤 다음 named window sibling은 이전 definition body depth나 visual hanging indent를 상속하지 않고, 항상 `WINDOW` clause body depth로 복귀해야 한다. 그래야 multiline named window list가 canonical / idempotent 하다.

### 3.4 current operator RHS continuation policy

현재 trailing operator / continuation set:

- symbols: `:=`, `=>`, `=`, `<`, `>`, `<=`, `>=`, `<>`, `!=`, `+`, `-`, `*`, `/`, `%`, `||`, `|`, `^`
- single-keyword family: `AND`, `OR`, `IN`, `IS`, `LIKE`, `LIKEC`, `LIKE2`, `LIKE4`, `BETWEEN`, `NOT`, `EXISTS`, `MEMBER`, `SUBMULTISET`, `ESCAPE`
- paired keyword family: `IS OF`, `MEMBER OF`, `SUBMULTISET OF`
- trailing inline-comment continuation consumer는 별도 ad-hoc keyword list가 아니라 `structural header family ∪ operator RHS family`다. 즉 `JOIN`/`ON`/`USING`/`WINDOW`/`SELECT`/`SET` 같은 header carry와 `AND`/`LIKE4`/`MEMBER OF`/`:=` 같은 RHS carry를 같은 shared taxonomy 위에서 본다.
- 단, union이라고 해서 precedence가 없는 것은 아니다. exact bare / completed owner anchor family와 operator RHS family가 같은 trailing token에서 충돌하면 owner anchor가 먼저고, generic operator RHS는 semantic family가 남기지 않은 residual operator path로만 해석해야 한다.
- header line이 trailing operator도 함께 가지는 경우(`WHERE col =`, `SET col =`, `SELECT expr +`, `RETURNING col =` 등)는 operator family만 따로 해석하면 안 된다. analyzer의 line-level continuation depth는 shared structural header family를 먼저 반영하고, 그 header family가 없을 때만 pure operator current-depth residual rule(`AND col =`, `v_total :=`)을 쓴다.

주의:

- `SELECT *`의 `*`는 projection marker이므로 operator RHS continuation으로 보면 안 된다.
- `MEMBER`/`SUBMULTISET`는 exact same-depth deferred-wrapper owner family가 아니라 generic operator RHS continuation family다. child query owner family(`IN`, `EXISTS`, `ANY/SOME/ALL`)와 같은 table에 뭉개면 안 된다.
- `MULTISET` 같은 generic expression owner는 현재 policy상 boundary helper 기본 집합에 넣지 않는다.

### 3.5 current render policy

현재 렌더링 정책:

- non-verbatim code line: `render indent = final_depth * 4`
- comment-only / comma-only line: 빌린 structural depth를 같은 폭으로 직렬화
- mixed leading-close code line도 line-level render indent는 `final_depth * 4`를 사용한다.
- raw/verbatim line만 원문 공백을 유지할 수 있다.

`4`는 현재 policy이며, 근본 원칙은 "render indent는 final depth와 configured indent width의 함수여야 한다"이다.

## 4. 구현 및 유지보수 규칙

### 4.1 한 줄 처리 순서

1. 의미 있는 open / close event를 lexical하게 식별한다.
2. 선두 close event를 먼저 소비한다.
3. 남은 토큰으로 owner/body/header/continuation을 분류한다. exact bare owner/header family와 inline comment split continuation은 별도 consumer로 다루고, generic leading-prefix classifier와 충돌하는 owner/header family에서만 bare taxonomy를 우선 참조한다.
4. 분류 결과를 활성 owner stack과 pending state 위에 투영한다.
5. 줄 끝에서 새 open event와 새 pending state를 반영한다.

### 4.2 split owner / header carry 규칙

- pending owner는 원래 owner depth를 그대로 들고 간다.
- owner header가 현재 줄에서 완성되어도 child body/query/open wrapper가 다음 줄에 시작하면 completed owner anchor를 유지한다.
- `MERGE WHEN ... THEN` split header는 header pending state가 `THEN`에서 body-depth token을 방출하기 전까지 branch body depth를 조기 확정하면 안 된다.
- `MERGE WHEN ... THEN` split header가 nested child query 안으로 들어가면 merge-header pending state는 child query line에서 line-local heuristic로 재구성하면 안 되고, child query가 닫힐 때까지 유지한 뒤 `THEN` 또는 이어지는 merge-header condition line에서 다시 소비해야 한다.
- plain `END` 뒤 qualifier/label split도 explicit pending state로 유지한다.
- qualified `END IF;`/`END LOOP;` 뒤 comment gap이 있어도, 다음 code line을 line-shape으로 추측하지 말고 retained scope state로 정렬한다.

### 4.3 lossy pending state 금지

- deferred query head
- completed owner anchor
- pure/mixed close align
- parenthesized `CASE` close
- standalone `(` wrapper carry

이런 상태는 "깊이 하나"만 들고 가지 말고, 필요한 의미를 보존하는 typed state로 유지해야 한다.

예:

- `owner align depth`
- `owner base depth`
- `next query head depth`
- `close align depth`
- `general paren floor`

### 4.4 새 owner family 추가 순서

새 owner family를 추가할 때는 아래 순서를 지킨다.

1. `sql_text` 분류 helper를 추가한다.
2. analyzer context/state에 반영한다.
3. formatter phase 2에 같은 semantics로 반영한다.
4. 이 문서의 3절 taxonomy / policy를 갱신한다.

검증 기준은 "같은 입력에서 analyzer와 formatter가 같은 구조 이야기를 하는가"다.
