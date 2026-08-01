--------------------------------------------------------------------------------
-- SPACE QUERY ORACLE SINGLE-STATEMENT FORMATTER FINAL BOSS III
-- Target: Oracle Database 19c+ (live certification uses Oracle Database Free 26ai)
--
-- A repeatable, side-effect-free statement covering a local PL/SQL function,
-- recursive SEARCH/CYCLE clauses, nested JSON_TABLE frames, GROUPING SETS,
-- PIVOT + UNPIVOT, analytics, KEEP, LISTAGG overflow handling, XML/JSON
-- construction, and exact parent-frame recovery after deeply nested lists.
--------------------------------------------------------------------------------
WHENEVER SQLERROR EXIT SQL.SQLCODE
WITH
FUNCTION canonical_code(p_text VARCHAR2)RETURN VARCHAR2 IS
BEGIN
RETURN UPPER(REGEXP_REPLACE(TRIM(p_text),'[^[:alnum:]]+','_'));
END;
params AS(
SELECT DATE'2026-04-01'AS start_day,5 AS day_count FROM dual
),
accounts(account_id,parent_account_id,account_name,segment_code,active_yn)AS(
SELECT 1,NULL,'Aurora Group','ENTERPRISE','Y'FROM dual UNION ALL
SELECT 2,1,'Bluebird North','GROWTH','Y'FROM dual UNION ALL
SELECT 3,1,'Cobalt South','GROWTH','Y'FROM dual UNION ALL
SELECT 4,2,'Dormant Lab','STARTER','N'FROM dual
),
account_tree(account_id,parent_account_id,account_name,segment_code,tree_depth,account_path)AS(
SELECT a.account_id,a.parent_account_id,a.account_name,a.segment_code,0,
CAST('/'||canonical_code(a.account_name)AS VARCHAR2(400))
FROM accounts a
WHERE a.parent_account_id IS NULL
UNION ALL
SELECT a.account_id,a.parent_account_id,a.account_name,a.segment_code,t.tree_depth+1,
t.account_path||'/'||canonical_code(a.account_name)
FROM accounts a
JOIN account_tree t ON t.account_id=a.parent_account_id
)
SEARCH DEPTH FIRST BY account_name SET traversal_no
CYCLE account_id SET cycle_yn TO'Y'DEFAULT'N',
calendar(day_no,day_value)AS(
SELECT 0,p.start_day FROM params p
UNION ALL
SELECT c.day_no+1,c.day_value+1
FROM calendar c
CROSS JOIN params p
WHERE c.day_no+1<p.day_count
)
CYCLE day_no SET calendar_cycle_yn TO'Y'DEFAULT'N',
events(event_id,account_id,event_day,event_status,payload)AS(
SELECT 101,1,DATE'2026-04-01','POSTED',
q'~{"channel":"web","metrics":[{"name":"revenue","value":120,"tags":["critical","new"]},{"name":"cost","value":45,"tags":["ops"]}]}~'FROM dual UNION ALL
SELECT 102,1,DATE'2026-04-03','POSTED',
q'~{"channel":"partner","metrics":[{"name":"revenue","value":80,"tags":["renewal"]},{"name":"cost","value":30,"tags":["ops","shared"]}]}~'FROM dual UNION ALL
SELECT 201,2,DATE'2026-04-02','POSTED',
q'~{"channel":"store","metrics":[{"name":"revenue","value":150,"tags":["critical"]},{"name":"cost","value":70,"tags":["field"]}]}~'FROM dual UNION ALL
SELECT 301,3,DATE'2026-04-02','POSTED',
q'~{"channel":"direct","metrics":[{"name":"revenue","value":200,"tags":["critical","bulk"]},{"name":"cost","value":90,"tags":["research"]}]}~'FROM dual UNION ALL
SELECT 302,3,DATE'2026-04-05','PENDING',
q'~{"channel":"partner","metrics":[{"name":"revenue","value":60,"tags":["forecast"]},{"name":"cost","value":20,"tags":["ops"]}]}~'FROM dual
),
metric_rows(event_id,account_id,event_day,event_status,channel_code,metric_no,metric_name,metric_value,tag_no,tag_value)AS(
SELECT e.event_id,e.account_id,e.event_day,e.event_status,j.channel_code,
j.metric_no,canonical_code(j.metric_name),j.metric_value,j.tag_no,j.tag_value
FROM events e
CROSS APPLY JSON_TABLE(
e.payload,'$'
COLUMNS(
channel_code VARCHAR2(30)PATH'$.channel',
NESTED PATH'$.metrics[*]'
COLUMNS(
metric_no FOR ORDINALITY,
metric_name VARCHAR2(30)PATH'$.name',
metric_value NUMBER PATH'$.value'DEFAULT 0 ON ERROR,
NESTED PATH'$.tags[*]'
COLUMNS(
tag_no FOR ORDINALITY,
tag_value VARCHAR2(30)PATH'$'
)
)
)
)j
),
metric_base(event_id,account_id,event_day,event_status,channel_code,metric_name,metric_value)AS(
SELECT event_id,account_id,event_day,event_status,MAX(channel_code),metric_name,MAX(metric_value)
FROM metric_rows
GROUP BY event_id,account_id,event_day,event_status,metric_name
),
event_pivot(event_id,account_id,event_day,event_status,channel_code,revenue_amount,cost_amount)AS(
SELECT event_id,account_id,event_day,event_status,channel_code,
NVL(revenue_amount,0),NVL(cost_amount,0)
FROM metric_base
PIVOT(
SUM(metric_value)FOR metric_name IN(
'REVENUE'AS revenue_amount,
'COST'AS cost_amount
)
)
),
round_trip_metrics(event_id,account_id,metric_name,metric_value)AS(
SELECT event_id,account_id,metric_name,metric_value
FROM event_pivot
UNPIVOT INCLUDE NULLS(
metric_value FOR metric_name IN(
revenue_amount AS'REVENUE',
cost_amount AS'COST'
)
)
),
daily(account_id,day_no,day_value,revenue_amount,cost_amount,margin_amount)AS(
SELECT a.account_id,c.day_no,c.day_value,
NVL(SUM(p.revenue_amount),0),NVL(SUM(p.cost_amount),0),
NVL(SUM(p.revenue_amount-p.cost_amount),0)
FROM accounts a
CROSS JOIN calendar c
LEFT JOIN event_pivot p
ON p.account_id=a.account_id
AND p.event_day=c.day_value
GROUP BY a.account_id,c.day_no,c.day_value
),
daily_analytics(account_id,day_no,day_value,revenue_amount,cost_amount,margin_amount,running_margin,previous_margin,margin_rank)AS(
SELECT d.*,
SUM(d.margin_amount)OVER(PARTITION BY d.account_id ORDER BY d.day_no ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW),
LAG(d.margin_amount,1,0)OVER(PARTITION BY d.account_id ORDER BY d.day_no),
DENSE_RANK()OVER(PARTITION BY d.account_id ORDER BY d.margin_amount DESC)
FROM daily d
),
grouped_metrics(account_id,metric_name,metric_total,grouping_id)AS(
SELECT account_id,metric_name,SUM(metric_value),GROUPING_ID(account_id,metric_name)
FROM round_trip_metrics
GROUP BY GROUPING SETS(
(account_id,metric_name),
(account_id),
()
)
),
account_rollup(account_id,event_count,revenue_amount,cost_amount,margin_amount,channel_list,top_tag,tag_json)AS(
SELECT p.account_id,COUNT(*)AS event_count,SUM(p.revenue_amount),SUM(p.cost_amount),
SUM(p.revenue_amount-p.cost_amount),
LISTAGG(DISTINCT p.channel_code,','ON OVERFLOW TRUNCATE'...'WITHOUT COUNT)
WITHIN GROUP(ORDER BY p.channel_code),
(SELECT MAX(r.tag_value)KEEP(DENSE_RANK FIRST ORDER BY r.metric_value DESC,r.event_id,r.tag_no)
FROM metric_rows r
WHERE r.account_id=p.account_id),
(SELECT JSON_ARRAYAGG(r.tag_value ORDER BY r.event_id,r.metric_no,r.tag_no RETURNING CLOB)
FROM metric_rows r
WHERE r.account_id=p.account_id)
FROM event_pivot p
GROUP BY p.account_id
),
final_rows(account_id,account_name,segment_code,tree_depth,account_path,event_count,revenue_amount,cost_amount,margin_amount,channel_list,top_tag,tag_json,peak_running_margin,account_xml)AS(
SELECT a.account_id,a.account_name,a.segment_code,t.tree_depth,t.account_path,
NVL(r.event_count,0),NVL(r.revenue_amount,0),NVL(r.cost_amount,0),NVL(r.margin_amount,0),
r.channel_list,r.top_tag,r.tag_json,
(SELECT MAX(d.running_margin)FROM daily_analytics d WHERE d.account_id=a.account_id),
XMLSERIALIZE(CONTENT XMLELEMENT("account",
XMLATTRIBUTES(a.account_id AS"id",a.segment_code AS"segment"),
XMLELEMENT("name",a.account_name),
XMLELEMENT("path",t.account_path)
)AS VARCHAR2(1000))
FROM accounts a
JOIN account_tree t ON t.account_id=a.account_id
LEFT JOIN account_rollup r ON r.account_id=a.account_id
)
SELECT
CASE
WHEN(SELECT COUNT(*)FROM events)=5
AND(SELECT COUNT(*)FROM metric_base)=10
AND(SELECT SUM(metric_value)FROM round_trip_metrics)=865
AND(SELECT COUNT(*)FROM calendar)=5
AND(SELECT COUNT(*)FROM account_tree)=4
AND(SELECT COUNT(*)FROM grouped_metrics WHERE grouping_id=3)=1
THEN'PASS'ELSE'FAIL'END AS status,
f.account_id,f.account_name,f.segment_code,f.tree_depth,f.event_count,
f.revenue_amount,f.cost_amount,f.margin_amount,f.peak_running_margin,
f.channel_list,f.top_tag,
JSON_OBJECT(
'path'VALUE f.account_path,
'tags'VALUE f.tag_json FORMAT JSON,
'xml'VALUE f.account_xml,
'totals'VALUE JSON_OBJECT(
'revenue'VALUE f.revenue_amount,
'cost'VALUE f.cost_amount,
'margin'VALUE f.margin_amount,
'grand'VALUE(
SELECT metric_total
FROM grouped_metrics g
WHERE g.grouping_id=3
)
)
RETURNING CLOB
)AS evidence
FROM final_rows f
ORDER BY f.account_id
/
