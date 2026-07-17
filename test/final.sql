--------------------------------------------------------------------------------
-- Oracle SQL / PL/SQL official-syntax executable certification suite.
-- Live target: Oracle AI Database Free 26ai (23.26.0.0.0).
-- Run from the repository root with SYSTEM connected to the FREE service.
--
-- References:
--   https://docs.oracle.com/en/database/oracle/oracle-database/26/sqlrf/
--   https://docs.oracle.com/en/database/oracle/oracle-database/26/lnpls/
--
-- This file is newly written and standalone: it does not include any existing
-- fixture. It creates only SQ_ORACLE_MANUAL_* objects in the connected schema,
-- validates all data it depends on, and can be run repeatedly.
--
-- SQ_ORACLE_MANUAL_TOPICS retains every SQL Language Reference TOC entry;
-- keyword and installed HELP catalogs are copied in full, and every statement
-- family is retained separately. LIVE executes a safe form; CATALOG_ONLY
-- covers infrastructure/file/option operations and global reconfiguration.
--------------------------------------------------------------------------------

SET SERVEROUTPUT ON SIZE UNLIMITED
SET FEEDBACK ON
SET VERIFY OFF
SET DEFINE ON
WHENEVER SQLERROR EXIT SQL.SQLCODE ROLLBACK

PROMPT [ORACLE FINAL] standalone schema, SQL, and PL/SQL coverage

BEGIN
  FOR ddl_text IN (
    SELECT 'DROP PROPERTY GRAPH sq_oracle_manual_graph' text_value FROM dual UNION ALL
    SELECT 'DROP MATERIALIZED VIEW sq_oracle_manual_mv' FROM dual UNION ALL
    SELECT 'DROP MATERIALIZED VIEW LOG ON sq_oracle_manual_log' FROM dual UNION ALL
    SELECT 'DROP VIEW sq_oracle_manual_v' FROM dual UNION ALL
    SELECT 'DROP SYNONYM sq_oracle_manual_syn' FROM dual UNION ALL
    SELECT 'DROP TRIGGER sq_oracle_manual_biu' FROM dual UNION ALL
    SELECT 'DROP PACKAGE sq_oracle_manual_pkg' FROM dual UNION ALL
    SELECT 'DROP PROCEDURE sq_oracle_manual_proc' FROM dual UNION ALL
    SELECT 'DROP FUNCTION sq_oracle_manual_fn' FROM dual UNION ALL
    SELECT 'DROP TYPE sq_oracle_manual_obj FORCE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_gtt' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_lob PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_topics PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_syntax PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_statements PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_keywords PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_metric PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_edge PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_node PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_child PURGE' FROM dual UNION ALL
    SELECT 'DROP TABLE sq_oracle_manual_log PURGE' FROM dual UNION ALL
    SELECT 'DROP SEQUENCE sq_oracle_manual_seq' FROM dual UNION ALL
    SELECT 'DROP DOMAIN sq_oracle_category_domain' FROM dual
  ) LOOP
    BEGIN
      EXECUTE IMMEDIATE ddl_text.text_value;
    EXCEPTION
      WHEN OTHERS THEN NULL;
    END;
  END LOOP;
END;
/

CREATE DOMAIN sq_oracle_category_domain AS VARCHAR2(32)
  CONSTRAINT sq_oracle_category_domain_ck CHECK (LENGTH(TRIM(VALUE)) > 0)
  DISPLAY UPPER(VALUE);

CREATE SEQUENCE sq_oracle_manual_seq
  START WITH 100 INCREMENT BY 1 MINVALUE 100 CACHE 20 NOCYCLE NOORDER;
ALTER SEQUENCE sq_oracle_manual_seq INCREMENT BY 1 CACHE 10;

CREATE TABLE sq_oracle_manual_keywords TABLESPACE users AS
SELECT keyword,
       length,
       reserved,
       res_type,
       res_attr,
       res_semi,
       duplicate
FROM v$reserved_words;

-- Exact topic inventory from the official Oracle 26 SQL Language Reference TOC.
CREATE TABLE sq_oracle_manual_topics (
  manual_ref VARCHAR2(256) CONSTRAINT sq_oracle_manual_topic_pk PRIMARY KEY
) TABLESPACE users;

INSERT ALL
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('index.html')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Preface.html#GUID-0897B474-6033-4398-AA8A-922F1C5CAF53')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Preface.html#GUID-20414789-A9BC-4B4D-9418-6EB6BA49CDD3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Preface.html#GUID-E409CC44-9A8F-4043-82C8-6B95CD939296')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Preface.html#GUID-B9D62D1F-68E6-4864-8E9B-0473347E53BC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Preface.html#GUID-0FC6A924-4908-4EB6-A2FC-CA20A4CDCD36')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Changes-in-This-Release-for-Oracle-Database-SQL-Language-Reference.html#GUID-0B18172E-8876-40D0-84DE-53C2CE6436BD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Changes-in-This-Release-for-Oracle-Database-SQL-Language-Reference.html#GUID-89203F7C-527E-4C9E-B628-9AE0F955F4A7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Changes-in-This-Release-for-Oracle-Database-SQL-Language-Reference.html#GUID-3C11D3A9-8B14-4DCC-B212-B7FE57EE81E8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Changes-in-This-Release-for-Oracle-Database-SQL-Language-Reference.html#GUID-2D6EBFD3-88AC-4AA0-B9C2-B3E41A8F30FB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Changes-in-This-Release-for-Oracle-Database-SQL-Language-Reference.html#GUID-26784915-C38B-4B35-909F-E02B414BACB9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Introduction-to-Oracle-SQL.html#GUID-049B7AE8-11E1-4110-B3E4-D117907D77AC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('History-of-SQL.html#GUID-4DD5E1B6-BEC7-4E9B-B369-1466F93ACA28')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Standards.html#GUID-BCCCFF75-D2A4-43AD-8CAF-C3C97D92AC63')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Standards.html#GUID-92B38403-6934-4E86-9D9A-E94957ACDDFC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Standards.html#GUID-454CD72B-6EE2-4D0B-8E7E-0573AC8D8388')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Using-Enterprise-Manager.html#GUID-ABEC85E5-1C69-40EE-BAE5-B693C1F2131C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Lexical-Conventions.html#GUID-D9AEB31A-8584-4066-85D0-AF6EFA609381')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Tools-Support.html#GUID-F54D11BA-BAAE-4285-94F8-6D706A2D936B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Basic-Elements-of-Oracle-SQL.html#GUID-41D065C3-3449-4DAE-B2D8-4DF256FFC88A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-A3C0D836-BADB-44E5-A5D4-265BA5968483')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-7B72E154-677A-4342-A1EA-C74C1EA928E6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-1BABC478-FB47-4962-9B0C-8B8BD059E733')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-85E0A0DD-9E90-4AE1-9AD5-93C89FDCFC49')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-FE15E51B-52C6-45D7-9883-4DF47716A17D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-0DC7FFAA-F03F-4448-8487-F2592496A510')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-DF7E10FC-A461-4325-A295-3FD4D150809E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-CC15FC97-BE94-4FA4-994A-6DDF7F1A9904')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-9401BC04-81C4-4CD5-99E7-C5E25C83F608')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-75209AF6-476D-4C44-A5DC-5FA70D701B78')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-10D4D073-866D-4BD4-B3E9-ED153D505A6A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-F579F4B8-EF13-4CAF-9B06-03B076861C41')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-CFE7487C-A4D0-4E90-A836-2697C45BDD10')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-12FE5221-9B49-4110-8D16-BF51BCED5562')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-33A52FDB-BA5C-474E-96D3-40390BA5F5F4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-4C0B65DB-E751-4957-A1ED-5044BAFA7812')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-F6309DF8-162F-48A4-9454-FEE59EC6644F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-7690645A-0EE3-46CA-90DE-C96DF5A01F8F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-5405B652-C30E-4F4F-9D33-9A4CB2110F1B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-918FA867-140C-4B78-8691-86448E8802F2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-94A82966-D380-4583-9AF1-AEE681881E64')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-BE23545B-469A-4A57-8D13-505F2F5DB706')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-E7CA339A-2093-4FE4-A36E-1D09593591D3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-ED59E1B3-BA8D-4711-B5C8-B0199C676A95')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-B03DD036-66F8-4BD3-AF26-6D4433EBEC1C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-E405BBC7-DA9A-4DF2-9F22-E60CB9EC0705')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-F8686599-B7AE-477D-8A58-FA0AA8B2C348')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-9B4F00D0-821A-4342-95AA-30CA43DBA124')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-4FD497DD-3331-4C25-9147-3CEBEFDBFF22')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-1A71C635-188E-4EC9-B821-1DBEC2B45451')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-3D9CC018-1637-45CB-95CF-DE67319D1A54')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-4570CDFD-8F91-44B9-BE7F-13076AA2AEBF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-0EAC5929-0674-429C-AF42-2D454C982F8F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-AB053D2C-2A40-478E-82E5-B9176C8776FD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-E441F541-BA31-4E8C-B7B4-D2FB8C42D0DF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-8EFA29E9-E8D8-40A6-A43E-954908C954A4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-285FFCA8-390D-4FA9-9A51-47B84EF5F83A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-801FFE49-217D-4012-9C55-66DAE1BA806F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-4231B94A-97E9-4B59-91EB-E7B2D0DA438C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-AEF1FE4C-2DE5-4BE7-BB53-83AD8F1E34EF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-E9F3AE1C-AA6D-4262-A15F-778833251361')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-0BC16006-32F1-42B1-B45E-F27A494963FF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-7CF27C66-9908-4C02-9401-06C2F2C4021C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-1E278F1C-0EC1-4626-8D93-80D8230AB8F1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-C9818949-BB51-4EB1-9A6D-2BE1F53B105D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-EAA3885B-06AA-4F0D-85E7-C43352E5E2AC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-FE91D392-FD57-4BEB-BE53-41D0E4D44264')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-B3300D21-4598-4AE5-AA95-451E9F1040ED')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-5A8C5AC6-BC32-4D78-B0DE-037162106C72')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-CBC6D668-4FDB-40C9-B240-DFDA6420C13B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-2FCFAF23-DFE9-4D05-8518-88AB134E0692')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-44E56F9F-3079-45AE-8744-A8069D38210E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-BF935A5E-3E6C-42C0-AA18-05D3A268D7D8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-639B3C49-AE4A-43F2-91BF-19BD53FE8193')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-6C9AC925-4E3F-476D-BB63-5A70CC12FC40')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-1CA616C7-BFA7-4AFC-A199-7589DA049CB6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-B4DF3B59-1600-4FA2-B7ED-AF7B734256BF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-022A5008-1E15-4AA4-938E-7FD75C594087')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-66AF10E5-137D-444B-B9BC-89B2B340E278')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Types.html#GUID-CFEFCFAC-4756-4B90-B88D-D89B861C1628')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-1563C817-86BF-430B-99AB-322EE2E29187')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-84DD4733-168C-4C67-BE9A-E4B12B3BA14C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-DAF46B4C-58C3-468B-8C16-3EFCF363ADB5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-0F560FC8-3ADF-4AFD-87BA-E4673EFEE9FA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-A114F1F4-A08D-4107-B679-323DC7FEA31C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-F29BC3C8-11FF-4712-97F3-C7F0487C0D09')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-E3D9DEDA-DEE7-4C35-A4F3-C113634F9BF2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-82C9B166-5850-48F5-B3ED-BB82B3631407')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-6DB331B5-0F34-4215-9A20-16AEA9D7FF4B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-4C49C87F-F170-43CC-9EDC-2403576610DF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-98BE3A78-6E33-4181-B5CB-D96FD9DC1694')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-7F3A7937-B057-4815-A7BB-DA5E7EBB66D5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-D0C5A47E-6F93-4C2D-9E49-4F2B86B359DD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Type-Comparison-Rules.html#GUID-6A02902A-1EF1-41E4-9494-381488BD272F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-192417E8-A79D-4A1D-9879-68272D925707')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-1824CBAA-6E16-4921-B2A6-112FB02248DA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-F521FBA0-FFED-4079-ABC4-9052218B3FD5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-A0B9C440-5C3C-407B-8C8B-0553293C7983')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-083FEFEA-B33F-436B-AEBF-9101A49EF189')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-8F4B3F82-8821-4071-84D6-FBBA21C05AC1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-DC8D1DAD-7D04-45EA-9546-82810CD09A1B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-4C258D8F-3DF2-4D45-BE3E-14864DD77100')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Literals.html#GUID-49FADC66-794D-4763-88C7-B81BB4F26D9E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-DFB23985-2943-4C6A-96DF-DF0F664CED96')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-24E16D8D-25E4-4BD3-A38D-CE1399F2897C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-096CA64F-1DA3-4C49-A18B-ECC7518EE56C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-49B32A81-0904-433E-B7FE-51606672183A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-22F2B830-261E-4BF0-91FB-6A1DAFC6D0A3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-4EF054FF-9996-4637-8476-136FC7A8246D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-EAB212CF-C525-4ED8-9D3F-C76D08EEBC7A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-E118F121-A0E1-4784-A685-D35CE64B4557')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-44E8F6D0-7532-4BE1-9300-F9775D9DB027')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-6C75461E-2E18-4C35-9EB4-038A7E1C9C1F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-914E090C-CE1C-406E-9DAC-1541B84E14CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-515DFB76-E853-432F-BFEC-F1C62306BEC5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-7FF68D9E-C7E2-4CA1-9DDB-5CC7169EEEEA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-A4CCA8BD-1679-432E-96BA-22FB46FF23E0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-5B755E80-3CB2-4901-BBCF-F0FC764E0BB5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Format-Models.html#GUID-C6EB69EF-74DB-4C10-A02E-210D31616552')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Nulls.html#GUID-B0BA4751-9D88-426A-84AD-BCDBD5584071')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Nulls.html#GUID-29B6554B-C948-4A8E-81C1-696A5128AAAD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Nulls.html#GUID-CDD7FB76-FC1B-492E-8431-871624FADA27')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Nulls.html#GUID-C785FE74-5F9C-4F70-AC4B-CA5D3010162A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-79B6B8FD-2DD4-471E-B9E0-0C8D20B058F6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-5C84C344-CEB3-4DBF-B748-337DE11CCE2A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-4527D8A1-89F9-4CF3-91C6-5857371D4074')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-D316D545-89E2-4D54-977F-FC97815CD62E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-06C047B1-133B-4211-9949-5FE9C0659D44')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-EAC8556A-3160-42F0-8101-4AA41BF119CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-562DD503-2F99-448E-B044-737BE726B58A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-DD069661-D431-40F5-9303-DB8C1153D87D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-EECD7FE0-A8FE-4B85-91EF-7984BC77392C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-BE83A338-FE21-444F-8CD9-455FC79C0057')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-3F26577F-8D14-41BD-BB02-7A45998FDA19')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-583D4C60-2B0E-4740-82D9-DD6F7EC38D9D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-814D718B-C5CC-4A5E-A697-BBBC53D6FF1B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-28F7F1DB-E265-40CB-BF41-E07A1A755566')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-377022EE-3F17-417E-84FE-AE87FA5C2653')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-0E97E9FE-9AE1-456A-89C3-019FB10E78E7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-DDABFD2E-FE80-4A2D-99D2-87E61EFA2182')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-72207153-4785-45D6-B1AA-CDB78D685FD1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-9B25245A-9884-4031-BAE0-538B4206194E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-A0F83FA9-BEDC-4343-97EC-500AD201AB5A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-CEA66568-0168-4AE2-AD3B-FE86CDD73894')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-5EF4198B-50B3-40D8-B12A-3D3115C69D9B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-581F6C91-1395-4ED0-81DE-59AE168FE183')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-6624DB8F-F071-4FA5-8B40-BDDBB4FC7214')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-9693C230-2616-4123-A1ED-3C41E9566F7A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-B5B05B54-DB85-40D2-8332-AA88C03ADBA6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-20390275-91A7-49DC-AAD1-A1FE943A4F75')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-EC0D9F8A-20E7-4281-A16A-6B9C993F2930')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-6FBA20C6-E981-4A1A-AAEA-131D9C4B6E62')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-ED589B14-7A6B-4CEB-9F6F-410E9DFF6BA2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-6D36EF83-F808-4056-896B-BCACAB83B8BD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-1D21818E-EA7E-4546-870D-B0C5CBD797CC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-59A98AD5-94EE-48D6-BD84-CE8986E4BAE1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-FCEE077B-6D56-48E5-A642-64EFDA4228AA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-E880F47F-06FD-4959-9D57-2D783A40D89D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-1F3EA36C-4F8F-453D-A66E-EB8FD2E85516')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-08F70BF0-4975-446B-B29C-6215F6510668')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-CAE5C861-7A10-45C4-89DB-340D83A60758')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-13330ACB-2F40-4927-82EA-A5D98FFE85F4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-C18CDA01-D24C-4861-AA10-C57DF20C7E0F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-4E431B5D-F61B-4F66-B86C-E9C8660E2FE7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-7C4C7728-B202-413B-9F91-EC1B8D63F3B0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-CD628749-C7E0-4EBB-A989-E9C10325EDF7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-02071A47-C4B0-4139-B985-4ED6E13B78F2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-86B4C4B6-28F6-4A11-8146-67ABE8331646')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-45FB9EE0-5CF1-42D7-8137-F2B9689369AE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-48179E54-E6DB-462B-BD8D-978B3340CFD1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-D4C251F0-B3A3-4D7E-843E-3491608D48DB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-141AB5C2-2ACF-4069-879C-4379F1AB0391')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-D7034ED9-BCCD-4B2C-8680-250E1FC0463A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-0CE4D80E-76A5-4082-A2D1-5FB7D7F67366')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-47B18C7A-E9D4-4BEF-A566-C23A84440F02')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-98FC989A-BC21-4D56-AB4A-2FD73B365DCC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-3A8628DA-7DFE-4B90-BDB0-9C98BE709376')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-54B0B0E9-5CB8-451C-B94D-1925CBC86BCA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-DAB72B9A-0141-4EC3-8877-348C53BCDC03')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-9F1C50DB-E9C7-4391-AE94-04D2A1E79BE5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-BEA2DE06-77C6-49AB-9EFB-8BD9469E8649')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-C0F71904-C245-4B53-8B1B-8113372FD5E1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-AF66D0C2-230F-4E0D-8ACB-F509491731E8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-5951911D-D611-40FF-BD0D-9B789D384313')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-6293C7D4-3571-4EF1-B8AE-B6F4442A1477')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-FE79056A-ED00-423F-8683-8055A5640356')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-EAB1F2B4-DF52-4261-9980-8E4CA3131F33')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-ED064FDD-61BC-4894-AAA7-A593154CABF4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-7DC64CF1-465E-4307-B5CF-62E0757FCECA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-99A00ADE-9D5A-47C9-9C35-A5D95ACC5A3B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-46C06086-1E94-4E0A-9A71-E4DE9386E364')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-E3514C53-015B-4FF3-8C63-F37F3A14D8AF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-0490CA0D-AECD-4FE1-861F-CDB20E6BDC34')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-142D6FFF-A4D9-4B85-8372-ABA662B8BC28')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-F2C789CC-DA58-4848-BC9E-3FCF761787D9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-0C6D937B-4C70-44AC-A3BF-570AEA897F9B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-AD766C93-F601-48E3-A339-BCA7604B10D3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-3406DC08-3D03-483B-8EFA-9D8E33AAEB2D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-7F2CC062-A31A-43E2-9FC7-0054CB936FF0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-47CB599E-F9A8-419E-BA25-6C97B9D6B27D')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-9C16655C-150E-4DA1-88E0-0ED8CADCCBA5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-F6AB55F9-5638-4698-8DB3-28CCFACE0178')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-2D5833D9-E618-4AA6-A665-9706451DEDD7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-382A0A51-6D6A-42A6-8CAB-3EA2606421BA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-99F14625-AA9B-4195-9573-28CD008E7352')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-DAC6E490-19FB-4140-A0A3-6CC60DD3D3A9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-02A8C120-F4D7-434C-829F-CF8118336FA1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-92EB25DD-5F00-4223-B39F-52B2AEB0D5CE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-D25225CE-2DCE-4D9F-8E82-401839690A6E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-D8952989-DB41-4F43-91E0-F727507E341F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-7694D1CD-2851-49EF-8C52-91BCB5C48A40')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-B04E477E-7080-4D02-A660-79EB71BDE980')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-191963B5-08FF-4C02-AB11-4424CC50CF19')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-C35DAFD1-B0F0-4849-B17C-5CC3638DD56C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-78B69F30-7062-429A-8D78-425E58DF04F9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-57609780-0DD4-4F29-8333-6301BCACEB53')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-56AB4452-3578-4391-A3AE-86E5AD46D377')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-33072CBF-E61E-4A6D-A118-55290B8B6578')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-F4B4ECC4-E1FE-40FB-AE05-D7B8AA001986')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-4664D3D8-6312-4C15-8E8F-4872DD7A44F8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-F9B87932-5149-4CA6-9FAF-E66410E66F5C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-A493DC0E-8F08-433D-B3C8-19AC163DB57E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-E7FEA4C2-4344-4E6E-871C-4E17AF0EB8F3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-9F03EB3B-382E-4B11-97E9-D7FC14CF92E7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-EE3246B9-3060-4F84-8C49-4524844E6B9C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-5A12BDC8-4CD1-448F-80BF-9F02653A3F94')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-D0514F31-CA97-402C-8BA4-931F8BFC62CB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-FA1147B3-BCAA-41F9-B6A2-8DEDABF1C021')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-BBBD2148-71AC-4CD7-80FA-F2ED3072A9A9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-56DAA0EC-54BB-4E9D-9049-BCEA934F7A89')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comments.html#GUID-F630F1F6-E5FC-448E-91DC-9A4D953C1114')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Objects.html#GUID-31BE00A7-7FF9-41CB-852A-F1416912CA9E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Objects.html#GUID-1B1818AD-6A70-4C2A-8E86-98BECA723FB8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Objects.html#GUID-A0B5BD29-5D01-4946-B19E-D7EC89AC6F65')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Object-Names-and-Qualifiers.html#GUID-3C59E44A-5140-4BCA-B9E1-3039C8050C49')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Object-Names-and-Qualifiers.html#GUID-75337742-67FD-4EC0-985F-741C93D918DA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Object-Names-and-Qualifiers.html#GUID-05F1B577-C08C-4DB9-925A-8799C76ADFF4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Database-Object-Names-and-Qualifiers.html#GUID-7B764856-864D-428D-BB8C-929BE8069806')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-1164C6E0-ABAB-49C2-8821-6B6C5047FEDD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-08B73ED6-2ABA-4737-B8A1-F7BD0456AEDB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-1822C16B-47E9-4690-862F-8E6A85F7D2EA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-61D0B206-A5EA-473F-AD04-7067D6E3914C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-CB1CA5F9-D8E8-4A62-B343-DAF66C9B9199')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-3B6E39A1-A538-4A8C-A314-932851A22777')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-DAF7C095-2C80-4E77-A01A-C0693D1070E7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-400255FF-792A-42D9-8BC4-BA5B88F71338')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-537665D2-E232-4DAC-9640-B9C2A1340D91')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-FED2E424-3F06-4B2B-88D2-DE043CA6E0E4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Syntax-for-Schema-Objects-and-Parts-in-SQL-Statements.html#GUID-30AFCEEA-2BC7-4C84-AC14-292BA44D3103')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Pseudocolumns.html#GUID-6C65C788-76AA-4A51-B011-51D53DD2521D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Pseudocolumns.html#GUID-2F2FBA6F-2FD1-47D6-A74F-DB4B31E4D400')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Pseudocolumns.html#GUID-DA181C6B-7B13-41E3-AAF5-5C19963D9D1C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Pseudocolumns.html#GUID-7DFBC564-31A6-4C71-A79C-6D3CC0B0BB10')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Pseudocolumns.html#GUID-D91FFF59-ECB0-40F0-AB4C-7A9D27EBEEF1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Sequence-Pseudocolumns.html#GUID-693B576A-191D-45F5-B7CB-88D0EA821B44')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Sequence-Pseudocolumns.html#GUID-D438E28B-3E30-4B12-8D52-8DA5CFE2E0FF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Sequence-Pseudocolumns.html#GUID-55228D7B-9CF1-4496-8524-3CD1DD4773FD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Version-Query-Pseudocolumns.html#GUID-F4DB0235-43A9-4AA2-8E9C-F2D9699D4AAD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COLUMN_VALUE-Pseudocolumn.html#GUID-66AD602D-7207-4BDF-9CB0-E7418CCC81D3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('OBJECT_ID-Pseudocolumn.html#GUID-EA125CCC-B4EE-4065-996E-12A1ADCC5F7F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('OBJECT_VALUE-Pseudocolumn.html#GUID-456B90CD-30DE-4973-98E0-E4B531938E6E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_ROWSCN-Pseudocolumn.html#GUID-8071AAB0-F656-4C93-B926-0BCE1439F121')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ora_shardspace_name-pseudocolumn.html#GUID-598FDFBE-7544-46B6-B307-DDA4102D3208')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROWID-Pseudocolumn.html#GUID-F6E0FBD2-983C-495D-9856-5E113A17FAF1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROWNUM-Pseudocolumn.html#GUID-2E40EC12-3FCF-4A4F-B5F2-6BC669021726')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLDATA-Pseudocolumn.html#GUID-EBB52EE8-57B4-4DCA-A17E-351DE5CFA934')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Operators.html#GUID-874EFABC-F473-44A3-BC93-CDCAC28B131A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Operators.html#GUID-CF1DBF8D-966F-4E5E-8AC8-9BF777B984D8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Operators.html#GUID-6A0C265F-3A7E-4E1C-8F79-8C6BCA26CFBA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Operators.html#GUID-FEF44762-F45C-41D9-B380-F6A61AD25338')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Arithmetic-Operators.html#GUID-46CD9FD8-FC94-44BA-AA62-30A16063EAAE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COLLATE-Operator.html#GUID-1B8CE3B0-77FC-455C-8400-6F81CF188D7B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Concatenation-Operator.html#GUID-08C10738-706B-4290-B7CD-C279EBC90F7E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Operators.html#GUID-4CC13EEB-846A-4254-93FC-E91E678BD302')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Operators.html#GUID-95F6A554-C6FE-42CD-88A6-7A1C162ED964')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Query-Operators.html#GUID-875C8985-4AEF-4DF1-BA23-3CDF5BCBCD8E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Set-Operators.html#GUID-5CB549AF-5A4F-453E-B164-49CAC8F94CBF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Operators.html#GUID-793FCBB0-A97C-4884-BCAC-DD0542EA746B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Operators.html#GUID-FCDB466F-08D0-4539-AFBB-34D4D2176C44')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Operators.html#GUID-B0C85A24-7E7A-4793-B5C1-F8F0222B45E6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Operators.html#GUID-12124160-B10B-4FE8-A850-4CE01FBD2384')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('shard_chunk_id-operator.html#GUID-FABB2038-EFA8-4A5C-8048-2B3F01D0E6CA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('User-Defined-Operators.html#GUID-6025E56E-8429-42E2-B5A6-6048B5D1AF25')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('data-quality-operators.html#GUID-30540D17-AC84-45F5-A511-75D95F7B0229')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('data-quality-operators.html#GUID-C13A179C-1F82-4522-98AA-E21C6504755E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('data-quality-operators.html#GUID-4D870366-C06F-4E63-BE15-609C1F2A96D3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph_table-operator.html#GUID-CA6A600E-2087-46F8-A081-C6F3F01CF305')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-reference.html#GUID-873488F0-58D1-4B9D-94B1-F4967B1785DD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-1F1E8BC1-CEBB-43A2-B66A-C7D9BB24D88C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-0C83E320-23F7-41C6-87A8-BE7582185789')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-C8045246-F087-46E4-A707-5BF3D5712196')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-590F45CA-2D6E-42D8-925E-28A9F1B421E3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-B6D2B840-7BC4-4F95-B9D1-B65B7EB826E6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-C364215A-E792-4CF3-A352-EA4C2ABC7A29')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-03FAEC9D-A5FE-4D8F-9F20-38556D08932B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-9671FB35-E95A-40F9-9ABB-7DE879AC46F5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-9AC5083D-1B05-4D62-9C5B-E14AD66C2C10')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-111F9A60-3554-46E6-93F1-BC88BF5E1949')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-395B9EB2-AB62-42B7-8CE5-9ADF2462B2C5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-pattern.html#GUID-6CD2D743-2D7E-4DF7-8E4D-4680F3CB2AF1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-table-shape.html#GUID-93202EC2-D2F3-45B0-869D-4C03057EB1A5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-table-shape.html#GUID-1C95A975-EEC8-44B0-AAE4-655B69F528E2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graph-table-shape.html#GUID-5B1619AA-A70A-4DDD-A3AC-2D1B19B93029')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-30B0844D-329B-4A95-BEA1-953AF9F3ED7C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-C93B0F61-CC3D-4071-A649-35DD8E31CE78')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-B9E06987-AB0B-4578-A462-2A6B5BF085EA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-8365A435-E525-4CE5-8600-11D203C5C971')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-3CA85A62-A083-4D12-9EFE-CF127BD8A3CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-4CC87ADB-39A9-4A4B-8D12-8E9E667DAEC9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-943AFEF7-986A-4510-906E-F53485EE145D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-6372EFF8-91D5-4C0F-821A-9461EAB5044B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-EAAF4274-26F3-45CB-907A-503C0105D1F0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-EBC833C9-F777-4708-827C-27EDB1719F79')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-EE2CEFE9-BFA1-4E86-8B7E-6B67B789D2E2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('value-expressions-graph_table.html#GUID-EA1322C1-C5B6-4414-A873-33F2DD0A193D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('json_id-operator.html#GUID-89A6BCC7-B500-429B-8299-A3F78D1D077F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Expressions.html#GUID-E7A5363C-AEE9-4809-99C1-1A9C6E3AE017')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Expressions.html#GUID-68789A5C-B142-496F-ADEE-837F75F95B2B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Simple-Expressions.html#GUID-0E033897-60FB-40D7-A5F3-498B0FCC31B0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('analytic-view-measure-expressions.html#GUID-F8C7ED67-A4EC-479C-975F-12F1F4B8CBA0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('analytic-view-measure-expressions.html#GUID-595B6AC1-9204-4119-890A-0C38F933D03C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Compound-Expressions.html#GUID-533C7BA0-C8B4-4323-81EA-1379657AF64A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CASE-Expressions.html#GUID-CA29B333-572B-4E1D-BA64-851FABDBAE96')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Column-Expressions.html#GUID-B16B2D82-5D4B-485B-AE20-160EC0C7137A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CURSOR-Expressions.html#GUID-B28362BE-8831-4687-89CF-9F77DB3698D2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Datetime-Expressions.html#GUID-F72A753A-98A4-4EBD-84E9-C014CE058384')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Function-Expressions.html#GUID-C47F0B7D-9058-481F-815E-A31FB21F3BD5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Interval-Expressions.html#GUID-EB9B5B5D-357B-494C-A237-153A2CF8425C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON-Object-Access-Expressions.html#GUID-09D1A154-335D-484E-A7A2-DA1983CD511C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Model-Expressions.html#GUID-83D3FD56-8346-4D3F-A49E-5FE41FE19257')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Object-Access-Expressions.html#GUID-FA69A056-12A6-420F-A106-EE252386CC43')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Placeholder-Expressions.html#GUID-B98B5394-A573-4BF8-9EC3-7B1BB1130553')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Scalar-Subquery-Expressions.html#GUID-475D80C3-C873-4475-AB1A-8837C5CF8CE4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Type-Constructor-Expressions.html#GUID-E8A491DE-18BA-4A1E-8CE2-BBA43E5C52D6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Expression-Lists.html#GUID-5CC8FC75-813B-44AA-8737-D940FA887D1E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('boolean-expressions.html#GUID-E492D339-5AAF-43C1-95B8-88DB1CDED0D9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Conditions.html#GUID-C2E3ED44-16E7-4924-9125-E1693B1022A8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Conditions.html#GUID-E9EC8434-CD48-4C01-B01B-85E5359D8DD7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Conditions.html#GUID-65B103FE-C00C-46A3-8173-A731DBF62C80')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comparison-Conditions.html#GUID-828576BF-E606-4EA6-B94B-BFF48B67F927')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comparison-Conditions.html#GUID-2590303E-81FE-4758-A971-1EE8B798951F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Comparison-Conditions.html#GUID-72CA75A4-AE94-471E-993F-20B969DB933F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Floating-Point-Conditions.html#GUID-D7707649-2C93-4553-BF78-F461F17A634E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Logical-Conditions.html#GUID-C5E48AF2-3FF9-401D-A104-CDB5FC19E65F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Model-Conditions.html#GUID-1F5B08DB-2B7A-4ECE-B51A-C753A426928B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Model-Conditions.html#GUID-759EA766-E377-4EC4-99C6-DE861E96CEDF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Model-Conditions.html#GUID-A26216BD-D937-412E-87B3-4B79F511AE38')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Conditions.html#GUID-E8164A15-715A-40A0-944D-26DF4C84DE3F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Conditions.html#GUID-1F61D1E7-4EA7-4254-8056-CB74ACFF254D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Conditions.html#GUID-EED3C932-8A77-4841-BCC0-CD524F1E65A1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Conditions.html#GUID-228D6708-37E1-4C54-8715-7EC2CF5B5998')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multiset-Conditions.html#GUID-6EC9172B-DF92-469B-B8BD-E7FFFCEFB37B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Pattern-matching-Conditions.html#GUID-3FA7F5AB-AC64-4200-8F90-294101428C26')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Pattern-matching-Conditions.html#GUID-0779657B-06A8-441F-90C5-044B47862A0A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Pattern-matching-Conditions.html#GUID-D2124F3A-C6E4-4CCA-A40E-2FFCABFD8E19')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Null-Conditions.html#GUID-657F2BA6-5687-4A00-8C2F-57515FD2DAEB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XML-Conditions.html#GUID-DE0B495D-F70A-4D37-AB8B-9376991E6081')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XML-Conditions.html#GUID-51E593FF-9AB0-4E1F-ABF7-5330F82FC0AE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XML-Conditions.html#GUID-37EF3738-5751-4888-9397-50EAD8360D6D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-JSON-Conditions.html#GUID-08C75404-6E58-4EBE-A8B4-0B6041B0DB63')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-JSON-Conditions.html#GUID-99B9493D-2929-4A09-BA39-A56F8E7319DA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-JSON-Conditions.html#GUID-35C7012D-FCDB-4106-88C1-CABA78326896')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-JSON-Conditions.html#GUID-57762C80-0C8A-4B18-9BA7-9B3F5ABDC988')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-JSON-Conditions.html#GUID-DEF7941B-1267-44E7-8514-5CD448503179')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Compound-Conditions.html#GUID-D2A245F5-8071-4DF7-886E-A46F3D13AC80')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BETWEEN-Condition.html#GUID-868A7C9D-EDF9-44E7-91B5-C3F69E503CCB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXISTS-Condition.html#GUID-20259A83-C42B-4E0D-8DF4-9A2A66ACA8E7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('IN-Condition.html#GUID-C7961CB3-8F60-47E0-96EB-BDCF5DB1317C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('IS-OF-type-Condition.html#GUID-7254E4C7-0194-4C1F-A3B2-2CFB0AD907CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('boolean-test-condition.html#GUID-E6611D82-5FC0-4466-A3F9-BA0E35F4103D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Functions.html#GUID-D079EFD3-C683-441F-977E-2C9503089982')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-SQL-Functions.html#GUID-D51AB228-518C-4213-8BD4-F919623D105E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Aggregate-Functions.html#GUID-62BE676B-AF18-4E63-BD14-25206FEA0848')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Analytic-Functions.html#GUID-527832F7-63C0-4445-8C16-307FA5084056')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Data-Cartridge-Functions.html#GUID-C2B7672A-A9B4-4B53-A9FB-08B9B2EB75D4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Model-Functions.html#GUID-C3070477-FF37-4FF1-8602-031163FB2646')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Object-Reference-Functions.html#GUID-538CB642-0FDA-4090-9197-8685E1B55EC6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('OLAP-Functions.html#GUID-2AE523A7-630C-4907-B91B-89861C141EBD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-B93F789D-B486-49FF-B0CD-0C6181C5D85C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-AC0E8A99-5097-4147-8295-C88EAC5AA362')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-C27DB967-BA88-4033-85E3-49F22C27ACD9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-06062705-1EC8-44ED-89B8-0F0573B74EA2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-07B646B7-FCED-4682-B160-B94A3E1272B0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-6D1746FB-E1A3-4377-8C76-1DEDFF561370')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-C6D2C400-F4F1-4D8D-861D-63814E3F6024')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-5652DBC2-41C7-4F07-BEDD-DAF620E35F3C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-D16F7FB3-48D9-4354-A58A-357515D402DC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-0E5115DD-F906-4F04-BB70-DF62DD4BBF91')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-292D5A19-3B8C-43C4-AD26-66952698EF7A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-57E0EC6A-4C7A-48B5-95E1-F47F2FF66993')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-B3E664DC-2675-4AC7-885B-B9AB287CF76F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-E64F8D20-C7E2-482A-914F-2781D0AA4E64')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-C64CC0DE-0D7C-42C8-B078-92A2984AD953')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-C13171B3-C070-4137-AC71-7A30BD26F380')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-CDCB89CB-CCE4-4AAB-84DC-A63725A733EF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-C4201DFA-90C5-46DA-B528-0B6D4E8C647A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-82B2A2E3-A7DD-46EC-9260-8EC320DA518E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-AEF8F898-493F-4BE8-86E6-06241BB78AB0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-C0C477F1-8210-4CA9-A5FA-0A340C409892')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Single-Row-Functions.html#GUID-97F3185F-B39A-492A-AD01-8CBCD4713AC9')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ABS.html#GUID-D8D3489A-44EA-4FEC-A6F0-B5E312FFC231')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ACOS.html#GUID-B4C70DD5-B908-4130-975A-6CFD5C1AC1F9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ADD_MONTHS.html#GUID-B8C74443-DF32-4B7C-857F-28D557381543')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ANY_VALUE.html#GUID-A3C47D5E-B145-40B2-93D2-CA3BA65C2D81')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_COUNT.html#GUID-7D07E04A-3F9A-425E-BADE-EDA9C6162E9C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_COUNT_DISTINCT.html#GUID-50055A05-0187-4481-AFE5-2414F7227713')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_COUNT_DISTINCT_AGG.html#GUID-EEDA9388-A066-422A-B5C0-639A3076A10B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_COUNT_DISTINCT_DETAIL.html#GUID-8FBD2881-743D-425E-A104-472A720DEF50')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_MEDIAN.html#GUID-F6A11DF2-121A-4057-9D0B-BF1A221B5622')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_PERCENTILE.html#GUID-70D54091-EE2F-4283-A10B-1AB5A1242FE2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_PERCENTILE_AGG.html#GUID-72A1DAB0-4A3E-42BF-9E20-92273AD62E11')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_PERCENTILE_DETAIL.html#GUID-F9A0B9B5-671F-43CA-9FA7-69A2DD174F54')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_RANK.html#GUID-4F20978C-3188-4225-863D-0F7A25FD78FD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('APPROX_SUM.html#GUID-AC2A72A7-24E5-4FB8-B012-BD35CB560D6B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ASCII.html#GUID-871D4171-FF70-475E-BC82-9B8F46239A5D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ASCIISTR.html#GUID-B6128485-4E86-4851-860F-AC03981E2388')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ASIN.html#GUID-809ACB4E-9FDA-4943-B234-DDB32522A523')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ATAN.html#GUID-12E8F1AA-54D0-4A19-8648-27094946C588')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ATAN2.html#GUID-D34E671B-F3C0-4390-A2D8-ABB702B4B5D3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('AVG.html#GUID-B64BCBF1-DAA0-4D88-9821-2C4D3FDE5E4A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BFILENAME.html#GUID-1F767077-7C26-4962-9833-1433F1749621')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BIN_TO_NUM.html#GUID-BF061402-D7F0-4557-B7D4-1CEE6E80F3B2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BITAND.html#GUID-EADBED75-6AC5-4FBE-991A-E3B4D260F73B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BIT_AND_AGG.html#GUID-82497098-6D77-48D3-89EF-C1041BF8A258')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BITMAP_BIT_POSITION.html#GUID-B57660B6-FDFA-4339-ADD3-DBE818C37BE6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BITMAP_BUCKET_NUMBER.html#GUID-6368CC51-B7C5-4BD9-9276-DE449BBC2CF3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BITMAP_CONSTRUCT_AGG.html#GUID-AD768466-C6FF-45D7-AB26-5B1315E76B84')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BITMAP_COUNT.html#GUID-F1A3261E-7DA9-4FEF-8413-E38907C856BA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BITMAP_OR_AGG.html#GUID-8C99C362-BE95-4495-9E50-4C1B572CCBEF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BIT_OR_AGG.html#GUID-18B0E3CB-1C90-4625-8E36-B422FA4E04A8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('BIT_XOR_AGG.html#GUID-1563FB7E-9CC9-4D03-859E-BE336AF01F1D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('boolean_and_agg.html#GUID-AF3C1A26-C7A1-4BD2-B15C-86B761D4D8D9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('boolean_or_agg.html#GUID-C3E187DE-BD26-4440-B0AD-51342FFA4775')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('calendar.html#GUID-D7BC24DF-02AC-4DFE-83D5-8AB126DDB216')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('calendar_add_x_periods.html#GUID-A04972BD-DAC8-4AF5-B2F9-04701F45B716')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('calendar_since.html#GUID-57321AA3-B40F-486B-8528-045DFF363855')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('calendar_start_end.html#GUID-D229D49F-E9C7-4435-8A9F-B22037C79A9A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('calendar_x_of_y.html#GUID-40D7074F-DB0A-4828-8BF1-432D73D20C0C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CARDINALITY.html#GUID-11F978F8-1DD9-4D82-9FCF-2FC633D1C100')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CAST.html#GUID-5A70235E-1209-4281-8521-B94497AAEF75')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ceil-datetime.html#GUID-666629BE-AA15-4EA6-86A0-DF321AEFF3C0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ceil-interval.html#GUID-F774D359-1828-44D4-8F48-7550A06E7206')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CEIL.html#GUID-6DCC9AFB-9B80-4C27-AF63-5AA3B1E43660')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CHARTOROWID.html#GUID-F9C63933-F680-465D-AB22-6B8B882B5CF7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('checksum.html#GUID-3F55C5DF-F23A-4B2F-BC6F-E03B34B78BA8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CHR.html#GUID-35FEE007-D49C-4562-A904-041186AC8928')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CLUSTER_DETAILS.html#GUID-6E47A5A7-B73A-4D79-BAA5-BB7E3C173D0F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CLUSTER_DISTANCE.html#GUID-21E611E3-2F15-4DF1-B648-9A36E8D5CE4D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CLUSTER_ID.html#GUID-1B0D0954-5A57-409C-9E84-F3EE12712040')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CLUSTER_PROBABILITY.html#GUID-999A15BA-FEDD-4FA6-8F1B-C847C2FE51CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CLUSTER_SET.html#GUID-7B44CB7A-4783-4FE0-80D8-26AE88D6B060')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COALESCE.html#GUID-3F9007A7-C0CA-4707-9CBA-1DBF2CDE0C87')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COLLATION.html#GUID-70A694BA-C1A0-4F5A-9492-58A5943D9BDD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COLLECT.html#GUID-A0A74602-2A97-449B-A3EC-847D38D3DA90')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COMPOSE.html#GUID-A16E7D53-E7F8-46A6-B3F8-BA322D129019')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CON_DBID_TO_ID.html#GUID-9F38A14F-8E6A-4A4A-96D5-52E4480A8926')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CON_GUID_TO_ID.html#GUID-F93F257F-BB58-427D-9E19-A22E43DB288F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('con_id_to_con_name.html#GUID-85BFFF80-142A-43EF-B807-60890FB64F65')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('con_id_to_dbid.html#GUID-4305B1BC-5829-4AAD-B4DF-AEE17EB8F18D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('con_id_to_guid.html#GUID-2B5AB386-2254-46A6-83B6-ED0504008F03')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('con_id_to_uid.html#GUID-0C8239D7-5848-4C23-8680-E0B19490C48B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CON_NAME_TO_ID.html#GUID-714E0914-5018-4E32-AB1E-134FDD0B28FE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CON_UID_TO_ID.html#GUID-14BE69F3-8519-4676-90CD-374152981901')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CONCAT.html#GUID-D8723EA5-C93A-45C3-83FB-1F3D2A4CEAF2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CONVERT.html#GUID-C8BA0657-61C8-4964-A4CB-9292390853F6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CORR.html#GUID-E73AF5E2-38A4-436A-955C-5122C079F49C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CORR_A.html#GUID-B2DED35A-2ECE-4DF0-BDA4-28F28B7BCA23')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CORR_A.html#GUID-CD1CCD1A-11DD-4FB2-9D3B-342DAA96128B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CORR_A.html#GUID-C00D906D-9421-49FC-BF3B-148C51219108')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COS.html#GUID-C008F067-C6DC-4C13-9B7F-5A385415363A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COSH.html#GUID-A48CD625-5238-4259-9A1F-0FDBFD19841E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COUNT.html#GUID-AEF08B79-024D-4E3A-B362-9715FB011776')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COVAR_POP.html#GUID-D728D05F-D2E3-405C-986F-088B8353553A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COVAR_SAMP.html#GUID-7850B9E1-83A4-41CB-8F17-DCD2E2A70C95')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CUBE_TABLE.html#GUID-55CDE2F2-14ED-4F8F-B5BF-1566C0E18727')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CUME_DIST.html#GUID-B12C577C-A63C-4D19-8E18-FCCBBFBF8278')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CURRENT_DATE.html#GUID-96795097-D6F0-4288-90E7-9D7C49B4F6E5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CURRENT_TIMESTAMP.html#GUID-CBD42B84-869D-45C7-9FFC-001DD7712097')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CV.html#GUID-32E56E9C-4F59-486E-8E4C-F332284C5EA7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DATAOBJ_TO_MAT_PARTITION.html#GUID-195AC748-0C9E-4A68-B2BC-2411DE435375')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DATAOBJ_TO_PARTITION.html#GUID-B6F62AFF-0AE1-469B-98B1-589A2D07F3A3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('datediff.html#GUID-FCA7AC3D-83FE-4694-A802-326CB7D80351')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DBTIMEZONE.html#GUID-F2368F72-7065-462F-80B9-E115F5A48025')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DECODE.html#GUID-39341D91-3442-4730-BD34-D3CF5D4701CE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DECOMPOSE.html#GUID-3E772756-F12C-4827-99A5-F7CF4F11A25A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DENSE_RANK.html#GUID-BB66F574-09DF-4594-87A4-ABD83E8DC3FE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DEPTH.html#GUID-C0107BE4-0003-4329-9CCE-D0671B3F3538')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DEREF.html#GUID-E551FFE4-619F-40CE-8303-683EFA3EB28F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('domain_check.html#GUID-599390A5-1B96-4465-82CE-DBC2345A018B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('domain_check_type.html#GUID-9ED35142-A66C-4511-9DE1-B8BB4350DE41')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('domain_display.html#GUID-BF1D853F-5BA8-4E9E-B8EB-BF7502F11D20')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('domain_name.html#GUID-BFF71A4E-8FF2-407A-8661-C0A24D4E5487')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('domain_order.html#GUID-FC34F669-0BCA-4F8A-B911-4ACAA1F8F11D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DUMP.html#GUID-A05793C9-B35D-4BA7-B68C-E3693BCF47A5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EMPTY_BLOB-EMPTY_CLOB.html#GUID-551B5A7C-A03B-4B2E-80EF-DAA8574CF160')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('every.html#GUID-C34D8A50-3050-4F32-941A-8C2512DEC62D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXISTSNODE.html#GUID-71731B1A-99E5-4B82-8243-DEEE6704796F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXP.html#GUID-414FB4AE-03B5-41AD-AE33-E3755EFED0A0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXTRACT-datetime.html#GUID-36E52BF8-945D-437D-9A3C-6860CABD210E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXTRACT-XML.html#GUID-593295AA-4F46-4D75-B8DC-E7BCEDB1D4D7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXTRACTVALUE.html#GUID-20AB974B-7544-4F44-B539-787FB6145680')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FEATURE_COMPARE.html#GUID-3D4E179F-F5D2-4FCF-AB42-4D0C8CC7D514')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FEATURE_DETAILS.html#GUID-A42F313B-22C1-4CAC-BA3F-C418178D743F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FEATURE_ID.html#GUID-BA187F80-5F51-49F6-BB69-64422FB9FD90')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FEATURE_SET.html#GUID-55582346-F1D6-447E-851A-D4912982EB28')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FEATURE_VALUE.html#GUID-EC0E44D0-BE01-49F8-9E5A-72B500119877')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FIRST.html#GUID-85AB9246-0E0A-44A1-A7E6-4E57502E9238')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FIRST_VALUE.html#GUID-D454EC3F-370C-4C64-9B11-33FCB10D95EC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('fiscal.html#GUID-D125AC73-1BB8-4D27-854D-E9F6BAC647BA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('fiscal_add_x_periods.html#GUID-8527E3C5-F545-4E53-9941-C408B5487509')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('fiscal_start_end.html#GUID-387BA2A3-A2C9-4EB5-9FEC-F862B27DB01C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('fiscal_x_of_y.html#GUID-5CAD894A-0E98-4ED7-89A2-ED81F7A3328F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('floor-datetime.html#GUID-3EB4F1BA-9D18-437C-96BA-D3B0282DDE97')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('floor-interval.html#GUID-2339112B-EA60-46F6-9BDA-63C0A99B86A1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FLOOR.html#GUID-67F61AC7-C097-4397-A122-213157BF584F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FROM_TZ.html#GUID-84384FF7-6462-480C-BC40-60087016857B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('from_vector.html#GUID-AA60B3CB-FCB7-4944-9E06-976C272855B1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('graphql-table-function.html#GUID-3B8F3473-AE08-4126-8D95-636FA72A05A6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('GREATEST.html#GUID-06B88B22-8466-44B6-93C7-50B222122ECE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('GROUP_ID.html#GUID-3A5A9C15-1B67-4FD7-AC41-EE8349B2E834')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('GROUPING.html#GUID-82E6084A-0BDF-4587-A40E-36899783F073')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('GROUPING_ID.html#GUID-E20A5B8E-73B6-42FD-8AFB-DD3CD6D6DC61')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('HEXTORAW.html#GUID-8571556F-C219-4814-A854-9F01581FFBDF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('INITCAP.html#GUID-9FE9E0EE-D6B6-4C2C-BDEF-4FF4E1314560')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('INSTR.html#GUID-47E3A7C4-ED72-458D-A1FA-25A9AD3BE113')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ITERATION_NUMBER.html#GUID-C7B75092-475A-4AB3-8A7C-94C68704538C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('is_uuid.html#GUID-01803FC5-A637-498C-932C-DEB909115D97')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_ARRAY.html#GUID-46CDB3AF-5795-455B-85A8-764528CEC43B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_ARRAYAGG.html#GUID-6D56077D-78DE-4CC0-9498-225DDC42E054')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_DATAGUIDE.html#GUID-4CF32887-0F46-4925-8381-AE2B74343933')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_MERGEPATCH.html#GUID-2004F536-BE60-4457-A1A8-AB908FFF5399')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_OBJECT.html#GUID-1EF347AE-7FDA-4B41-AFE0-DD5A49E8B370')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_OBJECTAGG.html#GUID-09422D4A-936C-4D38-9991-C64101283D98')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_QUERY.html#GUID-6D396EC4-D2AA-43D2-8F5D-08D646A4A2D9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('json_scalar.html#GUID-F05BD523-F827-4A5F-9A82-8CBC2DB04E2E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_SERIALIZE.html#GUID-01B769C6-A7B3-4136-977F-63CA05963D21')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_TABLE.html#GUID-3C8E63B5-0B94-4E86-A2D3-3D4831B67C62')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_TRANSFORM.html#GUID-DD2A821B-C688-4310-81B5-5F45090B9366')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('JSON_VALUE.html#GUID-C7F19D36-1E75-4CB2-AE67-ADFBAD23CBC2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('json-type-constructor.html#GUID-2B598841-A327-4610-91B9-602F480A8314')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('KURTOSIS_POP.html#GUID-F820DFF7-B758-460E-AECC-053915069B9F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('KURTOSIS_SAMP.html#GUID-487DE503-A015-415F-B6CD-F9D095B91178')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LAG.html#GUID-68081CD0-72BE-4C0A-AA6B-AD39FFA7BCF2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LAST.html#GUID-4E16BC0E-D3B8-4BA4-8F97-3A08891A85CC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LAST_DAY.html#GUID-296C7C02-7FB9-4AAC-8927-6A79320CE0C6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LAST_VALUE.html#GUID-A646AF95-C8E9-4A67-87BA-87B11AEE7B79')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LEAD.html#GUID-0A0481F1-E98F-4535-A739-FCCA8D1B5B77')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LEAST.html#GUID-0198D71B-051A-41D9-8E9C-599E24692556')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LENGTH.html#GUID-8F97F652-5AE8-4457-AFD7-7A6F25551E0C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LISTAGG.html#GUID-B6E50D8E-F467-425B-9436-F7F8BF38D466')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LN.html#GUID-DCC9EDAA-D308-4145-8E05-8D06A5EF5F6F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LNNVL.html#GUID-FBCCE9B1-614E-45FA-8EE1-DFAA4F936867')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LOCALTIMESTAMP.html#GUID-3C3D1F29-5F53-41F2-B2D6-A3767DFB22CA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LOG.html#GUID-3739F356-A4A0-4D0D-A4EB-9725ACA05CD1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LOWER.html#GUID-C8682D4C-9BED-48AC-B73A-1D70BF307F48')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LPAD.html#GUID-0C27B59A-A6CF-43D3-BF4B-07A3D0F2CE20')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LTRIM.html#GUID-81B3D53C-0BBC-4485-B057-C8012CD6E40F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MAKE_REF.html#GUID-926B9963-5387-4781-88D5-A005334C1F2A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MAX.html#GUID-E5372020-A6DA-44BF-93BE-DA8C3F74CD01')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MEDIAN.html#GUID-DE15705A-AC18-4416-8487-B9E1D70CE01A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MIN.html#GUID-F7F04E18-1AD8-4D15-9491-4622AD847A74')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MOD.html#GUID-E12A3928-2C50-45B0-B8C3-82432C751B8C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MONTHS_BETWEEN.html#GUID-E4A1AEC0-F5A0-4703-9CC8-4087EB889952')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NANVL.html#GUID-3C094646-2A70-41F5-984C-9BC0FB31494A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NCHR.html#GUID-3A1BDD54-6C0B-4067-99C5-A439C0F8D561')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NEW_TIME.html#GUID-1D1CC7DE-CA2A-4BEC-B404-89FD19EE36AC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NEXT_DAY.html#GUID-01B2CC7A-1A64-4A74-918E-26158C9096F6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_CHARSET_DECL_LEN.html#GUID-5F0939C0-4AFB-4CEA-9899-BDE85B9B2F11')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_CHARSET_ID.html#GUID-733B03A0-CD66-4645-A323-401A176499E3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_CHARSET_NAME.html#GUID-5DCFB255-92AD-4E94-9344-73B7918C106C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_COLLATION_ID.html#GUID-69EA3869-28E3-4CF8-9678-CD4F9878EE99')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_COLLATION_NAME.html#GUID-24848987-2A02-4B09-A690-D3C87308FB3A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_INITCAP.html#GUID-42C1581B-B5AA-4D4C-A489-BC5B38A754FD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_LOWER.html#GUID-96944213-377E-461C-9F02-2DC4EC2B1649')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLS_UPPER.html#GUID-91D6302F-4DE2-49FA-8837-D46D3FD58DF8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NLSSORT.html#GUID-781C6FE8-0924-4617-AECB-EE40DE45096D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NTH_VALUE.html#GUID-F8A0E88C-67E5-4AA6-9515-95D03A7F9EA0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NTILE.html#GUID-FAD7A986-AEBD-4A03-B0D2-F7F2148BA5E9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NULLIF.html#GUID-445FC268-7FFA-4850-98C9-D53D88AB2405')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NUMTODSINTERVAL.html#GUID-5A7392A8-7976-4465-8839-A65EFF1A80B6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NUMTOYMINTERVAL.html#GUID-B98B21AA-44F7-4A9D-A646-6775A1D5F46D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NVL.html#GUID-3AB61E54-9201-4D6A-B48A-79F4C4A034B2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NVL2.html#GUID-414D6E81-9627-4163-8AC2-BD24E57742AE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ora_check_data_privilege.html#GUID-8651EFCA-429B-4824-882C-4FF442A473E2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_DM_PARTITION_NAME.html#GUID-F9ADE9AD-C306-42D1-8274-3F73C2FBAC19')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_DST_AFFECTED.html#GUID-EE288E4B-DE55-4104-813C-11E28F7B474A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_DST_CONVERT.html#GUID-3A991FB0-0E98-48F5-902F-55C6FCA8DA13')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_DST_ERROR.html#GUID-02FAF3EC-D90A-42FB-A212-513314AD774A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ora_end_user_context.html#GUID-F6509A23-9674-4CE8-B32B-BED2B604E103')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_HASH.html#GUID-0349AFF5-0268-43CE-8118-4F96D752FDE6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_INVOKING_USER.html#GUID-FAE7B186-C40D-48BB-A2C9-AB7EE3878BF1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ORA_INVOKING_USERID.html#GUID-91F09A40-96CD-4759-8EDF-4C54219E8E83')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ora_is_column_authorized.html#GUID-1BC8DDB5-DBC8-4384-9126-B52510EE8860')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PATH.html#GUID-91937F98-7718-4F39-9225-1E0229F11F0D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PERCENT_RANK.html#GUID-66A868F5-9EBA-482A-BF8C-09300B9EE165')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PERCENTILE_CONT.html#GUID-CA259452-A565-41B3-A4F4-DD74B66CEDE0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PERCENTILE_DISC.html#GUID-7C34FDDA-C241-474F-8C5C-50CC0182E005')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('POWER.html#GUID-D280B322-D2C3-46D0-8076-C88F16CBEDC2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('POWERMULTISET.html#GUID-34F3B1D1-4089-4A5B-AA2C-9C69A5C36E6D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('POWERMULTISET_BY_CARDINALITY.html#GUID-57423B5C-CD16-4B3C-A796-AAA0910EF261')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREDICTION.html#GUID-DA66A1C3-BFB2-43A1-A3FF-93D4A3DAB9C6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREDICTION_BOUNDS.html#GUID-C9478C25-8D31-4A39-99B8-AB66A6614795')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREDICTION_COST.html#GUID-2E58222D-FB7E-4CA2-BCAA-C932FCDEE890')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREDICTION_DETAILS.html#GUID-D7261A56-E729-4882-B48D-CDD343C53810')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREDICTION_PROBABILITY.html#GUID-0F309771-40A3-4E23-9A96-CD134C80F584')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREDICTION_SET.html#GUID-25AE84A7-C733-4BC5-8C57-2E5574C49AFC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PRESENTNNV.html#GUID-2FB61064-9A7C-49E5-8448-6636CC69837E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PRESENTV.html#GUID-201643DA-918F-4F68-BF80-FEAA7EBFD829')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PREVIOUS.html#GUID-75D5C320-ECE3-444A-86C1-A5637F4428AF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RANK.html#GUID-0950BD34-C994-41DA-A8F9-34B3FE53BBBA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RATIO_TO_REPORT.html#GUID-9D10C275-4341-435F-ACF4-767B9CCB7390')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RAWTOHEX.html#GUID-F86E3B5B-7FEE-47FD-A0C2-2FC55DC21C9E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RAWTONHEX.html#GUID-5657B113-24CE-4DC6-BD11-63135B7DB009')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('raw_to_uuid.html#GUID-F5948759-F523-47F6-B7E9-96A99154CA51')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REF.html#GUID-B7622138-6EB6-4203-B5E7-91CAD52E9DB1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REFTOHEX.html#GUID-3F8A9932-063D-4EF1-85B7-03D823F6AC09')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REGEXP_COUNT.html#GUID-5148AF2E-9CED-497D-A78D-3A7847A45276')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REGEXP_INSTR.html#GUID-D21B53A1-83E2-4722-9BBB-638470715DD6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REGEXP_REPLACE.html#GUID-EA80A33C-441A-4692-A959-273B5A224490')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REGEXP_SUBSTR.html#GUID-2903904D-455F-4839-A8B2-1731EF4BD099')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REGR_-Linear-Regression-Functions.html#GUID-A675B68F-2A88-4843-BE2C-FCDE9C65F9A9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REMAINDER.html#GUID-430D4C4A-5779-4EBB-90C5-4D7CA7E73556')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REPLACE.html#GUID-1A79BDDF-2D3B-4AD4-98E7-985B2E59DA6B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('retail.html#GUID-C5F26E1B-3E27-4320-A6BA-E5CE98500E04')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('retail_add_x_periods.html#GUID-BEAB7FBC-D52F-4865-82E0-1DFBA2D541F7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('retail_day_exists.html#GUID-F39B1201-7A6B-421B-8139-47E22DD31D0A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('retail_start_end.html#GUID-98363296-917D-4662-86FB-371237A6681D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('retail_x_of_y.html#GUID-B363A138-9CC1-40AC-B7EB-F72026123A66')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROUND-date.html#GUID-C6D342D0-6068-4986-A759-70EF4599EC41')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('round-interval.html#GUID-BE2A9358-55A6-432B-AC78-535A4D1A55F2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROUND-number.html#GUID-849F6C45-0D72-4464-9C0F-8B6822BA85E1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROUND_TIES_TO_EVEN-number.html#GUID-49919B6B-4337-4812-A248-B5D98F102DBD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROW_NUMBER.html#GUID-D5A157F8-0F53-45BD-BF8C-AE79B1DB8C41')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROWIDTOCHAR.html#GUID-67998E5B-376A-45B5-B20B-1A87E5D370C1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROWIDTONCHAR.html#GUID-3178A4DA-2534-4A93-A819-7C14208AE9B5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RPAD.html#GUID-064CFCAE-5902-49F9-800E-0AF311AEF595')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RTRIM.html#GUID-95A7DAFB-F7AB-48F4-BE24-64B3C7A840AA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SCN_TO_TIMESTAMP.html#GUID-BCB0C8EE-0E03-4A61-A41A-69975FAC1803')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SESSIONTIMEZONE.html#GUID-2A243878-C1C5-4B7C-81DE-D8B024796EAB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SET.html#GUID-533164AC-B4F0-4FCE-ADA4-85C925CB8D14')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SIGN.html#GUID-08B75521-B5F5-4658-A005-4B4441C82945')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SIN.html#GUID-2AF4895F-5D23-4165-89D5-B1D404ED99BF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SINH.html#GUID-1EB8626B-4D84-4EAD-BD23-1A97F186FD4A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SKEWNESS_POP.html#GUID-DF34158F-B681-4933-BA27-0A3885A9F43C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SKEWNESS_SAMP.html#GUID-E71D9AEC-0AAA-4A6C-BF70-29EE9AD8F7EC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SOUNDEX.html#GUID-9C43625B-70CA-4B43-AE22-5EC2A02192F8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQRT.html#GUID-E28C0B65-AAD8-4077-A82E-2FB4CD261CCA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STANDARD_HASH.html#GUID-4A68DACE-CFCF-443B-8651-B6CEAA7C4FD7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_BINOMIAL_TEST.html#GUID-3DDCDC0C-0DB2-479F-A6EB-E9FC0063ABF4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_CROSSTAB.html#GUID-AA0958AE-FF56-4970-B880-23426E0B7E6D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_F_TEST.html#GUID-9E2A91FC-5BB3-449A-810C-DA6CB52B56ED')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_KS_TEST.html#GUID-ADE2ACB3-C852-499F-8892-E4AA101EC80D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_MODE.html#GUID-10BDACE0-C435-4E3F-BC50-FD1A41C0F508')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_MW_TEST.html#GUID-77AF4F10-D4DC-45A9-94E8-F4F648F81222')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_ONE_WAY_ANOVA.html#GUID-CC614CE5-56CB-4A54-8571-6FEAD2D2E75F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_T_TEST_.html#GUID-B570D6F6-E4D7-4033-AC83-7E76F2E9CC2A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_T_TEST_.html#GUID-448CF6C8-3F3A-4AD4-868A-6EC31D34B61B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_T_TEST_.html#GUID-34C56EBD-F075-4203-9E70-723329FBD13F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_T_TEST_.html#GUID-93DB178B-55ED-4526-B676-07F93823484B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STATS_WSR_TEST.html#GUID-80A8A9A9-7CD9-4358-B628-6D67BD42BA5B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STDDEV.html#GUID-CA0C3B1F-1A4C-4CFB-ADAB-D90216C4E099')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STDDEV_POP.html#GUID-4F804DE5-7E20-4E08-A1BA-32DBB167B34B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('STDDEV_SAMP.html#GUID-7B2A708E-E73A-4CFE-978E-3F9C4BD37467')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SUBSTR.html#GUID-C8A20B57-C647-4649-A379-8651AA97187E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SUM.html#GUID-5610BE2C-CFE5-446F-A1F7-B924B5663220')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_CONNECT_BY_PATH.html#GUID-D25A0F86-B559-4090-9164-7A2C84D1E11E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_CONTEXT.html#GUID-B9934A5D-D97B-4E51-B01B-80C76A5BD086')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_DBURIGEN.html#GUID-ABA33BEB-F7B7-477B-9FF2-028D62768797')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_EXTRACT_UTC.html#GUID-C540A8C8-72B1-46AF-A9AA-18D011763AD8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_GUID.html#GUID-761E36B4-32DA-497D-8829-3D4653381F9B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_OP_ZONE_ID.html#GUID-947900CE-F4E0-43B5-B30C-4FDDA3913F17')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('sys_row_etag.html#GUID-46D84F68-2E6E-40B9-81CD-2701E300E417')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_TYPEID.html#GUID-4E3D45A1-7433-495D-9062-88505A1496E0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_XMLAGG.html#GUID-BEDD241D-360A-46A2-AEBF-C8B70E465D75')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYS_XMLGEN.html#GUID-1AC25984-F4AB-468E-BF53-561275AD44E8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYSDATE.html#GUID-807F8FC5-D72D-4F4D-B66D-B0FE1A8FA7D2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SYSTIMESTAMP.html#GUID-FCED18CE-A875-4D5D-9178-3DE4FA956516')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TAN.html#GUID-473E2008-5951-4FC8-A356-14D3D085B8AA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TANH.html#GUID-8DD0B75F-1BDB-4E41-8C6D-FB5B2908AF80')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TIMESTAMP_TO_SCN.html#GUID-58796E1A-9943-4966-96E6-78B636BD2859')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('time_bucket-datetime.html#GUID-7C869201-5BE5-4DBD-97A0-864C48EA4034')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('timestampdiff.html#GUID-B14CC8F8-0F94-4911-97F4-05F3AC8580B6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_APPROX_COUNT_DISTINCT.html#GUID-42A18FFB-C992-44A0-AC3E-F4BBF005846F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_APPROX_PERCENTILE.html#GUID-463702B2-9199-41ED-AE03-865CABAD3E23')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_BINARY_DOUBLE.html#GUID-0BA2E065-8006-426C-A3CB-1F6B0C8F283C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_BINARY_FLOAT.html#GUID-66A51BE2-BE4A-4B99-9C37-73B110452D27')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_BLOB-bfile.html#GUID-232A1599-53C9-464B-904F-4DBA336B4EBC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_BLOB-raw.html#GUID-C4308DB1-5BFE-48F0-99E5-9E03B80B4585')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('to_boolean.html#GUID-B4FA8F5F-DD2A-4BEA-946A-B3CA60509294')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_CHAR-bfile-blob.html#GUID-F12F3C5A-8E3C-4FE1-BD7D-4AC0B79EA5A5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('to_char-boolean.html#GUID-B9923922-AD87-4C7A-BC9A-3A3BC2D6AA2E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_CHAR-character.html#GUID-EC078E16-11FE-4ABE-AE05-DA9AC1B4BEBC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_CHAR-datetime.html#GUID-0C3EEFD1-AE3D-452D-BF23-2FC95664E78F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_CHAR-number.html#GUID-00DA076D-2468-41AB-A3AC-CC78DBA0D9CB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_CLOB-bfile-blob.html#GUID-FD7D58FE-B97C-4B75-85A9-5F82FB1DE96A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_CLOB-character.html#GUID-82E2FAD3-B0C8-4A06-A882-26211EE0524C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_DATE.html#GUID-D226FA7C-F7AD-41A0-BB1D-BD8EF9440118')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_DSINTERVAL.html#GUID-DEBB41BD-9438-4558-A53E-428CE93C05D3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_LOB.html#GUID-35810313-029E-4CB8-8C27-DF432FA3C253')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_MULTI_BYTE.html#GUID-58A9F91A-5B1E-4C14-8F48-046F176E2F4A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('to_nchar-boolean.html#GUID-4F0853D4-C661-4DF2-8B67-F5207F2A86CF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_NCHAR-character.html#GUID-539E9F5C-CB47-4BCE-B468-C34CF6BABDC5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_NCHAR-datetime.html#GUID-C40DBBC2-B9F2-49D8-8775-DDA99FF41EAC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_NCHAR-number.html#GUID-B0FA1B2F-3285-46C4-96DA-3C7AED48987C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_NCLOB.html#GUID-56CEB237-8515-4030-A5D5-016CBC5FA6BB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_NUMBER.html#GUID-D4807212-AFD7-48A7-9AED-BEC3E8809866')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_SINGLE_BYTE.html#GUID-36364630-C62C-46C5-B29B-EFE3DFB5AA6D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_TIMESTAMP.html#GUID-57E09334-E3CC-4CA2-809E-F0909458BCFA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_TIMESTAMP_TZ.html#GUID-3999303B-89CA-4AA3-9817-458F36ADC9DC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_UTC_TIMESTAMP_TZ.html#GUID-1728EE3E-EC0C-4FA8-B404-99C0A445CE82')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('to_vector.html#GUID-2CCAB607-A28B-43F7-A71D-9800C0B9A380')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TO_YMINTERVAL.html#GUID-5DEBA096-7AC3-4B18-A4BE-D36FC9BDB450')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRANSLATE.html#GUID-80F85ACB-092C-4CC7-91F6-B3A585E3A690')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRANSLATE-USING.html#GUID-EC8DE4D2-4F24-456D-A2E7-AD8F82E3A148')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TREAT.html#GUID-037C0CD3-C256-4A02-80E0-C6F15147C5BF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRIM.html#GUID-00D5C77C-19B1-4894-828F-066746235B03')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRUNC-date.html#GUID-BC82227A-2698-4EC8-8C1A-ABECC64B0E79')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('trunc-interval.html#GUID-7719AF9B-5593-4F2B-9B82-03C51AEA693D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRUNC-number.html#GUID-911AE7FE-E04A-471D-8B0E-9C50EBEFE07D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TZ_OFFSET.html#GUID-D2007072-34C2-4971-BD2B-64D93A3D7A31')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('UID.html#GUID-DFDC8E24-B911-4C42-B4B1-853E964D3644')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('UNISTR.html#GUID-AAF757DB-6E5D-4548-9E36-6B36BB0BD83E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('UPPER.html#GUID-0518FB26-7FE5-43B9-AB31-9352F9F6029C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('USER.html#GUID-AD0B927B-EFD4-4246-89B4-2D55AB3AF531')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('USERENV.html#GUID-AC3C8AEF-A988-41C4-9242-69B54E5941D2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('uuid.html#GUID-2A0ECCC2-3DA1-442F-AC9D-A6FE643F381D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('uuid_to_raw.html#GUID-B752F910-3319-46C1-AD8E-F29CDEF6D9E5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('VALIDATE_CONVERSION.html#GUID-DC485EEB-CB6D-42EF-97AA-4487884CB2CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('VALUE.html#GUID-BEB129A5-525F-4EEF-A79C-261954056234')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('VAR_POP.html#GUID-B62FB4A4-BD1F-47B0-B412-31A98B70C2E4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('VAR_SAMP.html#GUID-314D5831-0E26-4ABF-9F46-35F78F97DA52')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('VARIANCE.html#GUID-EC33717A-2509-402D-B3BB-7EECB2E4ED8B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector.html#GUID-8A63005B-5512-4D20-954C-7A9DA877FE4B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_chunks.html#GUID-5927E2FA-6419-4744-A7CB-3E62DBB027AD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_distance.html#GUID-BA4BCFB2-D905-43DC-87B0-E53522CF07B7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_distance.html#GUID-604A5B68-10AF-48F3-A84F-ED0B90624059')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_distance.html#GUID-2FD8BC27-7614-471F-A4F5-3ED52130A05A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_distance.html#GUID-2128DC1D-612A-444F-87D8-3D249CD8F12D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_distance.html#GUID-6AE745CF-93E7-4192-8F80-7B9853DF5B72')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_dims.html#GUID-010349D7-190D-430B-A798-ACC486E1036A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_dimension_count.html#GUID-C3D937E0-7F9F-4C21-A214-0CFA31472E67')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_dimension_format.html#GUID-354ACE80-7120-4D45-B2B0-AB1D86E3D37D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_embedding.html#GUID-5ED78260-6D21-4B6B-86E0-A1E70EFA11CA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_norm.html#GUID-41554068-9EB8-49E8-A771-4E666674DDA8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_origin.html#GUID-60AE1060-F4B7-4A0F-A100-2D415869F281')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_random-and-vector_random_per_row.html#GUID-0E0E2CAD-347C-4E44-A19D-F2D809283751')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('vector_serialize.html#GUID-9E3FFB34-F924-4C02-B35D-30B9FA1DA1A3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('VSIZE.html#GUID-CDDB2A17-9398-4AF8-96FB-4297DDA2665B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('WIDTH_BUCKET.html#GUID-5E9058E5-A91F-45ED-A90D-E21355D19A88')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLAGG.html#GUID-BCD1D755-5E26-4F73-BA22-521C30D275DA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLCAST.html#GUID-06563B93-1247-4F0C-B6BE-42DB3B1DB069')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLCDATA.html#GUID-FB517A52-6F1A-4D8D-B632-F91EFA606691')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLCOLATTVAL.html#GUID-AE3B6441-74D8-4033-900B-A578A79E5F0A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLCOMMENT.html#GUID-AECB7BCC-C60F-4E0C-BD9A-E52D8F1599C4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLCONCAT.html#GUID-CEEEF777-4C7D-41E4-9F69-69DE6D1B07C2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLDIFF.html#GUID-B7746C15-27FD-4CAF-87EA-49C0DFA1E935')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLELEMENT.html#GUID-DEA75423-00EA-4034-A246-4A774ADC988E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLEXISTS.html#GUID-3D0D90DB-3D4F-4685-AFF6-72B6250624B9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLFOREST.html#GUID-68E5C67E-CE97-4BF8-B7FF-2365E062C363')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLISVALID.html#GUID-012BB50C-30E4-46BA-8199-A8480453F79E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLPARSE.html#GUID-39A93E58-F06E-4633-A7BF-6CF27A53D9B6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLPATCH.html#GUID-C52DA494-2840-475B-871F-1EA071299894')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLPI.html#GUID-142604E3-7999-4803-9DF5-28BDC0701571')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLQUERY.html#GUID-9E8D3220-2CF5-4C63-BDC2-0526D57B9CDB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLSEQUENCE.html#GUID-BE0837A9-7D85-4621-8C22-1FECAD17E569')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLSERIALIZE.html#GUID-F2D5ECE7-3838-4DD5-BE8F-2AEE7890AA1C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLTABLE.html#GUID-C4A32C58-33E5-4CF1-A1FE-039550D3ECFA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('XMLTRANSFORM.html#GUID-3B74EED2-E79F-4333-8C0B-02989DF5EEAA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROUND-and-TRUNC-Date-Functions.html#GUID-8E10AB76-21DA-490F-A389-023B648DDEF8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-User-Defined-Functions.html#GUID-4EB3E236-8216-471C-BA44-23D87BDFEA67')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-User-Defined-Functions.html#GUID-06AF8D10-BBCF-4284-B2DB-61A86968578A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-User-Defined-Functions.html#GUID-3591F708-2397-47DE-928B-B0E792C990DB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-User-Defined-Functions.html#GUID-AACF85A3-B326-4EDD-9FCD-B51DB4377E37')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Common-SQL-DDL-Clauses.html#GUID-9B312612-AC5E-49FD-9223-005C6597C271')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('allocate_extent_clause.html#GUID-DA6B3DC2-84B5-4404-AD96-5ABF7341580F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('constraint.html#GUID-1055EA97-BA6F-4764-A15F-1024FD5B6DFE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('deallocate_unused_clause.html#GUID-016A106B-47D4-4FFF-8A3B-2DF19A5FE9FF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('file_specification.html#GUID-580FA726-F712-4410-90CF-783A2DA89688')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('logging_clause.html#GUID-C4212274-5595-4045-A599-F033772C496E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('parallel_clause.html#GUID-59C9EBF3-A45E-4EE5-ABE7-0DA0FCF6C4B5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('physical_attributes_clause.html#GUID-A15063A9-3237-43D3-B0AE-D01F6E80B393')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('size_clause.html#GUID-E97FADC2-A6E1-4D68-9F79-DCA271B86517')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('storage_clause.html#GUID-C5A67610-3160-41E9-8D48-03206BD5ED15')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('annotations_clause.html#GUID-1AC16117-BBB6-4435-8794-2B99F8F68052')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Queries-and-Subqueries.html#GUID-5937EB2B-D3EC-45D4-BF75-1FC02E45DAE2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('About-Queries-and-Subqueries.html#GUID-DB7521FE-9329-415E-B583-EA4467E990A7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Creating-Simple-Queries.html#GUID-DB044D5C-A960-4813-84DA-A1880C913339')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Queries.html#GUID-0118DF1D-B9A9-41EB-8556-C6E7D6A5A84E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Hierarchical-Queries.html#GUID-E3D35EF7-33C3-4D88-81B3-00030C47AE56')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('The-UNION-ALL-INTERSECT-MINUS-Operators.html#GUID-B64FE747-586E-4513-945F-80CB197125EE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Sorting-Query-Results.html#GUID-E45EF993-20AC-4552-860C-4D74EADB5BF2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-39081984-8D38-4D64-A847-AA43F515D460')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-A7F5091B-9C42-4FC3-8F2B-BB238518FA14')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-3AA5EB23-2D84-4E19-BD7E-E66A3C59D888')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-568EC26F-199A-4339-BFD9-C4A0B9588937')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-B0F5C614-CBDD-45F6-966D-00BAD6463440')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-70DD48FA-BF46-4479-9C3F-146C5616E440')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-794F7DD5-FB18-4ADC-9E46-ADDA8C30C3C6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-29A4584C-0741-4E6A-A89B-DCFAA222994A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-D688F2E3-7F1E-4339-894F-01A73E62328C')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Joins.html#GUID-E98C180E-8A17-469D-8E68-56245E28104B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Using-Subqueries.html#GUID-53A705B6-0358-4E2B-92ED-A83DE83DFD20')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Unnesting-of-Nested-Subqueries.html#GUID-DA7A69AA-156D-4F1B-9E29-DAE9D230D9B5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Selecting-from-the-DUAL-Table.html#GUID-0AB153FC-5238-4E79-8522-C9E2A04AB5E4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Distributed-Queries.html#GUID-DADE67A5-1C58-4132-9B2F-C14AE9B65508')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-ADMINISTER-KEY-MANAGEMENT-to-ALTER-JAVA.html#GUID-04D8EBB3-583D-4969-8344-E221000E9CD9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-E1749EF5-2264-44DF-99EF-AEBEB943BED6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-FD9A8CB4-6B9A-44E5-B114-EFB8DA76FC88')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-2E008D4A-F6FD-4F34-9071-7E10419CA24D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-FEC504E9-D22D-4082-A092-07891911C5CF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-B8AEC1B3-D1E8-4567-9EFB-8F3410CA70A4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-83CC2729-F33B-45D8-A6C5-0D3C654FBFC4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Types-of-SQL-Statements.html#GUID-CD76B6B5-01C4-46CA-964A-A41872D6AEB0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('How-the-SQL-Statement-Chapters-are-Organized.html#GUID-8B052D2F-D532-4F8E-8388-BCFEC30B65A8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ADMINISTER-KEY-MANAGEMENT.html#GUID-E5B2746F-19DC-4E94-83EC-A6A5C84A3EA9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-ANALYTIC-VIEW.html#GUID-5256BE3A-F134-40D4-8E70-684E073574C8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-assertion.html#GUID-D50F3CF9-5AC9-4041-988F-ACFEAC0987DD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-ATTRIBUTE-DIMENSION.html#GUID-F345D0F9-8133-4257-9A07-EDCE558A1332')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-AUDIT-POLICY-Unified-Auditing.html#GUID-CC41B5C2-09F4-40BC-B7FD-3B4C0A3F5437')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-CLUSTER.html#GUID-A4E03C13-7690-4567-9B0A-DA6A21173B4D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-DATABASE.html#GUID-8069872F-E680-4511-ADD8-A4E30AF67986')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-DATABASE-DICTIONARY.html#GUID-27DDB403-7E7F-40EC-9B48-5E3B475E27AE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-DATABASE-LINK.html#GUID-0259D771-9D04-4D86-A94D-61B621A3918A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-DIMENSION.html#GUID-16B451F9-FF21-4E44-ACCA-2CFFA6F3F0F9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-directive-validate.html#GUID-80608F72-7303-467B-B7D8-3534B888C9CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-DISKGROUP.html#GUID-22D73AB6-7063-4627-A2ED-18D521ED2557')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-domain.html#GUID-089B335C-EDC9-4AB6-894E-2EFEFE3F2BDA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-end-user.html#GUID-B09E7730-D204-4B1C-A7F1-33CE27E690DC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-FLASHBACK-ARCHIVE.html#GUID-285814C9-06ED-4BDB-BB19-E2BA6505C850')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-FUNCTION.html#GUID-6FB32876-2DB3-41EB-B0CA-91B163826AB2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-HIERARCHY.html#GUID-37A4E442-EE3A-4239-8228-F08A2F666D91')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-INDEX.html#GUID-D8F648E7-8C07-4C89-BB71-862512536558')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-INDEXTYPE.html#GUID-BFA7E29C-7905-4811-9119-B20FD8EA18F2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-INMEMORY-JOIN-GROUP.html#GUID-AF24F413-BB14-4B5D-93BF-9EB31ACFEBEC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-JAVA.html#GUID-6B211750-3247-4D71-9533-3DD8F66640CD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-json-relational-duality-view.html#GUID-F5360747-E268-4083-8D29-46073EA4D513')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-ALTER-LIBRARY-to-ALTER-SESSION.html#GUID-B4A1C1D6-D628-40F9-A6B0-A5FB3DB1D0C1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-LIBRARY.html#GUID-BB90AF66-3B1F-46C4-9716-4578DE0AE1F3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-LOCKDOWN-PROFILE.html#GUID-B4029154-54A8-4B78-97C3-9CED416F1C34')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-MATERIALIZED-VIEW.html#GUID-29EE5682-AE42-4879-ABAD-E34E66ADD233')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-MATERIALIZED-VIEW-LOG.html#GUID-4DAD5E6F-E30A-43D0-B023-634752E0E627')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-MATERIALIZED-ZONEMAP.html#GUID-9330FD16-28B6-4B22-8205-FF59AF250C1A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-mle-env.html#GUID-AF0D1253-6FEF-44A7-BEA3-9F24AEFF17C1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-mle-module.html#GUID-9CE0DFB6-68BE-427D-AEEB-294C0FD31F8F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-OPERATOR.html#GUID-F00A0AC8-36C8-4EAC-A9BB-B3D42C5EEEDE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-OUTLINE.html#GUID-49F25C82-0783-4407-88BB-613F986C2FEC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-PACKAGE.html#GUID-47471C4C-03AB-4D78-A295-3D58C91102E0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-PLUGGABLE-DATABASE.html#GUID-A29491AD-8F0F-4E52-9D94-57FC3FF8FBC7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-pmem-filestore.html#GUID-330BB1D5-5194-4B09-93AF-B80DBD36774A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-PROCEDURE.html#GUID-24A5796F-7D97-49D4-8448-7E541CB73AC6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-PROFILE.html#GUID-7D2EA18A-49F9-40FA-ADE8-BB3D5D5FE4A1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('alter-property-graph.html#GUID-3437140C-6244-49A2-9305-FBEB4D414761')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-RESOURCE-COST.html#GUID-92DCB41E-5113-4722-8A54-E90E1AE7DB54')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-ROLE.html#GUID-85543272-EAF4-4FED-A921-AD9868102C39')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-ROLLBACK-SEGMENT.html#GUID-B25701E3-A074-44C4-9018-C6691BEB2483')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-SEQUENCE.html#GUID-A6468B63-E7C9-4EF0-B048-82FE2449B26D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-SESSION.html#GUID-27186B28-7EFC-4998-B1ED-2B905CC0211B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-SESSION.html#GUID-8DBA8659-413E-49B4-98D3-D9608C9C8026')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-SESSION.html#GUID-DC7B8CDD-4F89-40CC-875F-F70F673711D4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-ALTER-SYNONYM-to-COMMENT.html#GUID-7B9A6386-C065-4D0D-957E-9859DD917A6C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-SYNONYM.html#GUID-C31B6804-6783-4A8C-B448-DF78E3FE6837')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-SYSTEM.html#GUID-2C638517-D73A-41CA-9D8E-A62D1A0B7ADB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-TABLE.html#GUID-552E7373-BF93-477D-9DA3-B2C9386F2877')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-TABLESPACE.html#GUID-CA074861-55D3-4768-8995-43D4DA26365D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-TABLESPACE-SET.html#GUID-63FEDE73-C1F1-4B7A-98ED-8C34C4073549')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-TRIGGER.html#GUID-085BD628-2903-46A3-9850-C0D8ED7F2EEF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-TYPE.html#GUID-E0C4E28C-726F-4481-99FE-15AC67342DC9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-USER.html#GUID-9FCD038D-8193-4241-85CD-2F4723B27D44')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ALTER-VIEW.html#GUID-0DEDE960-B481-4B55-8027-EA9E4C863625')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ANALYZE.html#GUID-535CE98E-2359-4147-839F-DCB3772C1B0E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ASSOCIATE-STATISTICS.html#GUID-BD02BA6A-32A7-4093-A6B6-BAE860C0F834')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('AUDIT-Traditional-Auditing.html#GUID-ADF45B07-547A-4096-8144-50241FA2D8DD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('AUDIT-Unified-Auditing.html#GUID-B24D6874-4053-4E66-8238-6CD0C87E9DCA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CALL.html#GUID-6CD7B9C4-E5DC-4F3C-9B6A-876AD2C63545')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COMMENT.html#GUID-65F447C4-6914-4823-9691-F15D52DB74D7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-COMMIT-to-CREATE-JAVA.html#GUID-A087EE75-DE65-4AA6-A479-280413DB74C8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('COMMIT.html#GUID-6CD5C9A7-54B9-4FA2-BA3C-D6B4492B9EE2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-ANALYTIC-VIEW.html#GUID-EBA7E9BC-3F49-4AA7-9EF6-9255FE7AE466')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-application-identity.html#GUID-E17F974D-09B5-4B38-8266-190D7AA59ABB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-assertion.html#GUID-9FB74CAA-0CAD-4FE6-B873-1B3877CB8AB9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-ATTRIBUTE-DIMENSION.html#GUID-62722AB0-2136-4BC9-8E76-CBEA13C15196')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-AUDIT-POLICY-Unified-Auditing.html#GUID-8D6961FB-2E50-46F5-81F7-9AEA314FC693')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-CLUSTER.html#GUID-4DBC701F-AFC3-486D-AA32-B5CB1D6946F7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-CONTEXT.html#GUID-FDF62812-A884-479C-9C1B-5BD6DDEFE7FA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-CONTROLFILE.html#GUID-9B389F28-C4D0-405D-BFE6-48237E8BD791')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-data-grant.html#GUID-58B4AF50-3F80-401B-923C-6C2191210877')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-data-role.html#GUID-27A2E887-A3A1-4FBB-B6F2-23F4667CB58F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-DATABASE.html#GUID-ECE717DF-F116-4151-927C-2E51BB9DD39C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-DATABASE-LINK.html#GUID-D966642A-B19E-449D-9968-1121AF06D793')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-DIMENSION.html#GUID-E6CD4CFC-5D06-4A8F-9DF1-C609A7EB8413')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-directive-validate.html#GUID-999E67CB-44F0-4761-BEBE-7371A7F0F649')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-DIRECTORY.html#GUID-8E9C569A-1B06-42C4-9586-0EF83437001A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-DISKGROUP.html#GUID-039A1373-1F3F-4A53-A152-8EBC348FB880')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-domain.html#GUID-17D3A9C6-D993-4E94-BF6B-CACA56581F41')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-EDITION.html#GUID-6CF92CA1-CAF7-4967-9B34-C02D72C23617')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-end-user.html#GUID-7CBA526A-976A-4DD1-9423-40B9C33F3039')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-end-user-context.html#GUID-44462C80-CF57-4EC9-9E4C-669FCCA74C6C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-FLASHBACK-ARCHIVE.html#GUID-9E821EC5-8350-4729-85FE-2188EBB4139B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-FUNCTION.html#GUID-156AEDAC-ADD0-4E46-AA56-6D1F7CA63306')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-HIERARCHY.html#GUID-73925877-992B-4624-AA28-8F565E9C3F0D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-hybrid-vector-index.html#GUID-8CDFC950-44A1-4340-A9C0-07DB9A777867')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-iceberg-table.html#GUID-2D39FEA3-72D0-47AF-ABCA-5866731D55AE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-INDEX.html#GUID-1F89BBC0-825F-4215-AF71-7588E31D8BFE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-INDEXTYPE.html#GUID-4A7BD0EC-B3E5-4D1D-95C5-C8B52D01D8CE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-INMEMORY-JOIN-GROUP.html#GUID-87CA7034-4F80-4D46-8EE1-5CC865C2D676')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-JAVA.html#GUID-69E13452-1F91-4F98-B154-CF5B1C198387')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-json-relational-duality-view.html#GUID-64B579AD-BF97-4B27-BF22-94C1FB6FD6DF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-CREATE-LIBRARY-to-CREATE-SCHEMA.html#GUID-B0517B99-DDA2-4F47-8866-46AD944FABF8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-LIBRARY.html#GUID-F042ABC9-2BF5-4E65-9D52-216D6228B288')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-LOCKDOWN-PROFILE.html#GUID-1CDEC3A3-F3F1-4279-9370-36AACF416E0A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-logical-partition-tracking.html#GUID-4828A4B0-712A-4BD9-9684-5717E094D450')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-MATERIALIZED-VIEW.html#GUID-EE262CA4-01E5-4618-B659-6165D993CA1B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-MATERIALIZED-VIEW-LOG.html#GUID-13902019-D044-4B79-9EB4-1F60652D037B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-MATERIALIZED-ZONEMAP.html#GUID-1E5048FC-3D28-49BC-80FE-7871568B4702')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-mle-env.html#GUID-419C81FD-338D-495F-85CD-135D4D316718')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-mle-module.html#GUID-EF8D8EBC-2313-4C6C-A76E-1A739C304DCC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-OPERATOR.html#GUID-62676C58-6F57-4572-8C09-7984A8E3EE9F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-OUTLINE.html#GUID-7CC033AF-DB19-4616-87D9-8173939FD627')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-PACKAGE.html#GUID-40636655-899F-47D0-95CA-D58A71C94A56')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-PACKAGE-BODY.html#GUID-E571E5A3-1C4B-4246-BF26-0E4348BEB6D6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-PFILE.html#GUID-C01CBA1C-F477-49BE-AD58-F2FED046D561')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-PLUGGABLE-DATABASE.html#GUID-F2DBA8DD-EEA8-4BB7-A07F-78DC04DB1FFC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-pmem-filestore.html#GUID-2518DAF0-E174-4593-86C2-D8E48FBED1FE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-PROCEDURE.html#GUID-771879D8-BBFD-4D87-8A6C-290102142DA3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-PROFILE.html#GUID-ABC7AE4D-64A8-4EA9-857D-BEF7300B64C3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-property-graph.html#GUID-37364ADB-E89C-4D92-A431-F2544FEDB218')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-RESTORE-POINT.html#GUID-AD0FB693-7C28-4908-A870-BA884B320575')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-ROLE.html#GUID-B2252DC5-5AE7-49B7-9048-98062993E450')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-ROLLBACK-SEGMENT.html#GUID-14AE3104-5B33-4E53-8E6F-6B2F037B52E9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-SCHEMA.html#GUID-2D154F9C-9E2B-4A09-B658-2EA5B99AC838')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-CREATE-SEQUENCE-to-DROP-CLUSTER.html#GUID-01CD18EA-DF10-4B99-B64A-69BB959EEE59')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-SEQUENCE.html#GUID-E9C78A8C-615A-4757-B2A8-5E6EFB130571')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-SPFILE.html#GUID-D3E295B7-A3A4-43D3-8BBD-5CBE171A2E52')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-SYNONYM.html#GUID-A806C82F-1171-478E-A910-F9C6C42739B2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-TABLE.html#GUID-F9CE0CC3-13AE-4744-A43C-EAC7A71AAAB6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-TABLESPACE.html#GUID-51F07BF5-EFAF-4910-9040-C473B86A8BF9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-TABLESPACE-SET.html#GUID-877951F1-B2A5-4907-9F0F-EF4F1884E8C4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-TRIGGER.html#GUID-EE0DF3AA-7ADC-4171-B8E8-138BE9224E3B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-true-cache.html#GUID-9CDFE592-D927-427F-A997-B9A50B646A56')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-TYPE.html#GUID-E72E3EE6-DE95-4F58-8941-E2F76D0EAE80')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-TYPE-BODY.html#GUID-C4F1591A-6F62-4897-9039-2C3F066F1E9D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-USER.html#GUID-F0246961-558F-480B-AC0F-14B50134621C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('create-vector-index.html#GUID-B396C369-54BB-4098-A0DD-7C54B3A0D66F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('CREATE-VIEW.html#GUID-61D2D2B4-DACC-4C7C-89EB-7E50D9594D30')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DELETE.html#GUID-156845A5-B626-412B-9F95-8869B988ABD7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DISASSOCIATE-STATISTICS.html#GUID-6E9A7D93-E28A-469D-97AB-2BECC2EF3C43')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-ANALYTIC-VIEW.html#GUID-16BF7588-87E8-4324-BCEC-242355245720')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-application-identity.html#GUID-C04F6D3D-F57C-4939-9475-E3719F2DE76A')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-assertion.html#GUID-42ED6603-2225-4932-B26B-2300EC85D37C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-ATTRIBUTE-DIMENSION.html#GUID-98D6273D-5F83-4AEC-85AF-7540A710F59D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-AUDIT-POLICY-Unified-Auditing.html#GUID-811D3F84-744E-47A1-B69D-C9D2FA4A0844')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-CLUSTER.html#GUID-531F7DE2-AA2A-400E-BC9A-4CBEEA7B7156')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-DROP-CONTEXT-to-DROP-JAVA.html#GUID-F06DDEEA-AE25-4912-A8EB-E83F8251BD91')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-CONTEXT.html#GUID-1C5C56A8-A3A3-421B-BEC5-C6ECCA0B60D0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-data-grant.html#GUID-067163F6-2530-4BDE-AFB5-922CBFD02113')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-data-role.html#GUID-3A7767A5-27FA-4F2B-811D-BAE29AE60F0C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-DATABASE.html#GUID-4FFC1AF5-538D-4882-8979-7A9957492A23')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-DATABASE-LINK.html#GUID-89856C55-29FB-4B52-84A9-E53B8D115864')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-DIMENSION.html#GUID-658FB451-6759-4777-ACDB-614CFDEFDF80')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-directive-validate.html#GUID-54085857-48D6-45D9-9E6E-A3232A2D3FB4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-DIRECTORY.html#GUID-3719950A-7B6A-4284-8467-B3455ECF8516')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-DISKGROUP.html#GUID-6F77FABB-3365-448F-8E2B-9B776904182C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-domain.html#GUID-E82527C8-5C47-43D0-9C0D-E081E78E612F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-EDITION.html#GUID-726082F7-C931-4975-8F2B-5EA814A51AB0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-end-user.html#GUID-0771F59A-581E-40D1-85F7-BE81B0D704B4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-end-user-context.html#GUID-7D0E733D-61F5-4349-A6AC-534D8E4D2BD1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-FLASHBACK-ARCHIVE.html#GUID-FFF61E62-28AF-4F7B-BBD7-8D9AC08DDE77')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-FUNCTION.html#GUID-5BF63D1C-797E-4FB7-BEAB-B02BD7AADAEF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-HIERARCHY.html#GUID-03DD165D-EDCC-484F-B79B-D56447587669')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-iceberg-table.html#GUID-9876FF23-47FB-4281-875A-D2A329BAB671')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-INDEX.html#GUID-F60F75DF-2866-4F93-BB7F-8FCE64BF67B6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-INDEXTYPE.html#GUID-36D05D07-72C4-48F9-B27D-7C4BBD2C1A81')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-INMEMORY-JOIN-GROUP.html#GUID-520D0E9A-B577-4BCD-B6CB-8EB448C0686D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-JAVA.html#GUID-0D24ADCD-01C8-4FB0-B14C-F5D9FB25E321')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-DROP-LIBRARY-to-DROP-SYNONYM.html#GUID-A1F5C322-5D22-4989-91A7-59C8F3ECC419')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-LIBRARY.html#GUID-82F45872-78AD-4125-8D14-EE6A69E2738D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-LOCKDOWN-PROFILE.html#GUID-62D428C1-5081-43CA-B45D-7FF1B81363E7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-MATERIALIZED-VIEW.html#GUID-187B88E0-F84A-44DB-8F4D-F477586FD22B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-MATERIALIZED-VIEW-LOG.html#GUID-878A08F7-CB95-4911-BE2E-9FEED8861410')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-MATERIALIZED-ZONEMAP.html#GUID-818B228D-9849-4C0C-944E-7A8DEF04A2D0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-mle-env.html#GUID-19AD5DA2-10B0-46D7-BBEF-6313B6A79425')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-mle-module.html#GUID-1E3DEB27-76D6-4564-BC3F-B11DB02609A7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-OPERATOR.html#GUID-48E60BB4-9490-4B8B-B08E-D17EAEBDDD7E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-OUTLINE.html#GUID-776F36E0-1905-48DC-9062-FBFAD5E1C36F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-PACKAGE.html#GUID-FAE96214-2ED0-4130-8ACA-A077C7A90B23')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-PLUGGABLE-DATABASE.html#GUID-4A663783-E184-417A-8BE1-703E1CDBA30B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-pmem-filestore.html#GUID-BA62AE81-AA2A-444E-BB46-57B7FB526EFC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-PROCEDURE.html#GUID-D7F2B5AD-DEEE-466B-B6D3-B765EB897DCB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-PROFILE.html#GUID-2892A481-F2C8-4B62-8960-E593D1150D83')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('drop-property-graph.html#GUID-4B091B93-3681-4CB9-BFDC-A21000AD3637')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-RESTORE-POINT.html#GUID-5FC039A9-46C8-4604-8985-C29CB617798C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-ROLE.html#GUID-F5DEDFAF-EDE6-4733-8E17-C5B94E3168DB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-ROLLBACK-SEGMENT.html#GUID-26B4C9D6-EFB4-4523-B84D-FAD42060D3D4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-SEQUENCE.html#GUID-32B640EE-47C9-46A7-9746-6125BAF8FF8C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-SYNONYM.html#GUID-C7293D40-83B8-4E60-9E90-CB907F2CA6C7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-DROP-TABLE-to-LOCK-TABLE.html#GUID-4DF57957-21B8-4033-A87B-1F37F27FD572')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-TABLE.html#GUID-39D89EDC-155D-4A24-837E-D45DDA757B45')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-TABLESPACE.html#GUID-C91F3E94-4503-48DE-9BCA-42E495E6BE11')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-TABLESPACE-SET.html#GUID-B14EC4C4-87C2-4E79-AB1A-044B620DF1FE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-TRIGGER.html#GUID-724AC8BC-0428-43D3-8F11-4D4AD8DC2984')
SELECT 1 FROM dual;

INSERT ALL
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-TYPE.html#GUID-2CBA2EFD-1B01-46A8-A4CD-B2975D3A1D67')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-TYPE-BODY.html#GUID-9D661F88-2174-4D21-87CA-CC6A36385C05')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-USER.html#GUID-F766E1A2-6686-4734-89BA-0C5B4120B90E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('DROP-VIEW.html#GUID-1A1BD841-66B9-47E4-896F-D36E075AE296')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('EXPLAIN-PLAN.html#GUID-FD540872-4ED3-4936-96A2-362539931BA0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FLASHBACK-DATABASE.html#GUID-BE0ACF9A-BC13-4810-B08B-33326440258B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('FLASHBACK-TABLE.html#GUID-FA9AF2FD-2DAD-4387-9E62-14AFC26EA85C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('GRANT.html#GUID-20B4E2C0-A7F8-4BC8-A5E8-BE61BDC41AC3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('grant-data-role.html#GUID-C8ABC027-9A3B-4304-A3D9-981E30C35F19')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('INSERT.html#GUID-903F8043-0254-4EE9-ACC1-CB8AC0AF3423')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('LOCK-TABLE.html#GUID-4C00C6D9-C5C5-46CC-AD33-A64001744A4C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SQL-Statements-MERGE-to-UPDATE.html#GUID-07BBB875-6272-441A-893F-35E2F9CA58ED')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('MERGE.html#GUID-5692CCB7-24D9-4C0E-81A7-A22436DC968F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NOAUDIT-Traditional-Auditing.html#GUID-9D8EAF18-4AB3-4C04-8BF7-37BD0E15434D')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('NOAUDIT-Unified-Auditing.html#GUID-EB92BE04-B09C-493F-952E-9629E739900E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('PURGE.html#GUID-9257F773-E019-4464-80F4-F5AB61D7D9B6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('RENAME.html#GUID-573347CE-3EB8-42E5-B4D5-EF71CA06FAFC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('REVOKE.html#GUID-BAAD2331-40A5-4366-86CA-BAA6B957E866')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('revoke-data-role.html#GUID-24F7DE71-8A20-4732-9E86-65D27A684189')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ROLLBACK.html#GUID-94551F0C-A47F-43DE-BC68-9B1C1ED38C93')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SAVEPOINT.html#GUID-78EEA746-0021-42E8-9971-3BA6DFFEE794')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SELECT.html#GUID-CFA006CA-6FF1-4972-821E-6996142A51C6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SET-CONSTRAINTS.html#GUID-1EF5B212-17C5-4F7C-9412-D777DFDEDCE9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SET-ROLE.html#GUID-863F9B6F-82B4-4C49-8E3A-3BA33AE79CAB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('SET-TRANSACTION.html#GUID-F11E1E30-5871-48D1-8266-F80A1DF126A1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('set-use-data-grants-only.html#GUID-79E33139-70CE-4EB1-B1B2-4314458DFC05')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRUNCATE-CLUSTER.html#GUID-90C16956-644E-4E28-A53D-BB34ED630561')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('TRUNCATE-TABLE.html#GUID-B76E5846-75B5-4876-98EC-439E15E4D8A4')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('UPDATE.html#GUID-027A462D-379D-4E35-8611-410F3AC8FDA5')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('update-end-user-context.html#GUID-17072C19-4156-4C05-BCDB-6072083F8033')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('How-to-Read-Syntax-Diagrams.html#GUID-6DFBA035-DBCB-4F37-984D-37E9EBC1038B')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Graphic-Syntax-Diagrams.html#GUID-D22097D5-1E7A-4A17-862A-F0084732B3CE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Graphic-Syntax-Diagrams.html#GUID-6A602A21-B55A-48E7-8E76-A9A551E55A7E')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Graphic-Syntax-Diagrams.html#GUID-6FF77DC8-764F-4130-957F-73D7511CCD5C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Graphic-Syntax-Diagrams.html#GUID-3A6451B1-A646-4F18-BE86-0297EC6387DF')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Graphic-Syntax-Diagrams.html#GUID-98D4D8B3-76C2-4EF5-8EAB-E5A742C9173C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Backus-Naur-Form-Syntax.html#GUID-A4C08C40-8E2B-43F9-A2AA-9953288D4230')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-and-Manual-Locking-Mechanisms-During-SQL-Operations.html#GUID-0304C4AA-BD28-4C2A-B7F5-267532FB9499')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-Locks-in-DML-Operations.html#GUID-3D57596F-8B73-4C80-8F4D-79A12F781EFD')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-Locks-in-DDL-Operations.html#GUID-84D392A3-94EC-444D-950F-7829DBCD43EE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-Locks-in-DDL-Operations.html#GUID-6F6A5BE3-07B6-4AF4-9BA9-B856B6DB2118')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-Locks-in-DDL-Operations.html#GUID-4E9FB993-2948-4C42-A5C8-A692A662B781')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-Locks-in-DDL-Operations.html#GUID-C2606A88-DF43-44A8-A48F-25C6783487BC')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Manual-Data-Locking.html#GUID-B1DE7D59-7FD1-4971-B98D-B69529DF7688')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Automatic-and-Manual-Locking-Mechanisms-During-SQL-Operations.html#GUID-1B08DE66-5ED8-4BEF-893B-B887E3A82D50')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-and-Standard-SQL.html#GUID-330DEBBB-006E-4B35-A516-5C0AEFFE06B9')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ANSI-Standards.html#GUID-F51EA195-0669-4DED-9D81-B7205AAC642F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('ISO-Standards.html#GUID-BBCB4C70-C6CD-4AC7-A2C6-1D1B32732931')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-To-Core-SQL2011.html#GUID-D372D906-805B-49B8-824A-D4697B05B7F8')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Support-for-Optional-Features-of-SQLFoundation2011.html#GUID-3BA98AEC-FAAD-4F21-A6AD-F696B5D36D56')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-SQLCLI2008.html#GUID-C02FD016-90DB-408E-B9E8-AAB18582DBD7')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-SQLPSM2011.html#GUID-651F9066-1511-407B-A002-C04AB2F2A534')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-SQLMED2008.html#GUID-F484CC68-C6DF-4587-ACAB-1ACD313DCE43')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-SQLOLB2008.html#GUID-79118C02-7279-4582-BDFA-8EFF1EE8BB90')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-SQLJRT2008.html#GUID-DD68745D-EC08-48B1-AB5D-E24CAEE3AEC3')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-SQLXML2011.html#GUID-0D0F19C8-0FB7-4FDD-A55B-18839F340E17')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('oracle-compliance-sql-mda.html#GUID-19127687-62CB-445F-B384-D8E5FC6C4BBA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('oracle-compliance-sql-pgq.html#GUID-2984AEEA-BB05-4209-B144-57DB2B8D5878')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-FIPS-127-2.html#GUID-17C40E8F-D8E4-42BE-B552-9B6AB8A98CCB')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Extensions-to-Standard-SQL.html#GUID-7D3A5F6C-79D4-4B71-AEC3-7AB847DF0989')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Compliance-with-Older-Standards.html#GUID-738EFE64-D6DC-41E2-B1D8-29C706DC060F')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Character-Set-Support.html#GUID-7625CE74-9C1F-4CC8-A223-43B8D53776E0')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-Regular-Expression-Support.html#GUID-969230D6-FC1A-4C75-BF2A-6B1BE909DED6')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Multilingual-Regular-Expression-Syntax.html#GUID-B03DEEAC-3F9E-4BFD-89D5-F481EA391D7C')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Regular-Expression-Operator-Multilingual-Enhancements.html#GUID-8A02D839-90A5-4FB6-AC43-7AE8CB08E8BA')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Perl-influenced-Extensions-in-Oracle-Regular-Expressions.html#GUID-2D2B8DEB-1343-4DA3-BBC1-5A5C79A5FC20')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-SQL-Reserved-Words-and-Keywords.html#GUID-6A07BB21-AD82-4B47-80FA-9B1141CC23C2')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-SQL-Reserved-Words.html#GUID-55C49D1E-BE08-4C50-A9DD-8593EB925612')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Oracle-SQL-Keywords.html#GUID-82EA000B-5661-41EB-AAF7-6BDDB4AB58EE')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Extended-Examples.html#GUID-8FE85432-DF49-4C26-9785-1F1363FBE8B1')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Using-Extensible-Indexing.html#GUID-BEAC690B-1FA4-4B31-9B28-FEAF45A01665')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('Using-XML-in-SQL-Statements.html#GUID-5FE21EC9-1F66-45F1-9FD8-ECA5336EDC14')
  INTO sq_oracle_manual_topics(manual_ref) VALUES ('book-index.html')
SELECT 1 FROM dual;

CREATE TABLE sq_oracle_manual_syntax TABLESPACE users AS
SELECT topic AS syntax_topic,
       seq AS syntax_seq,
       info AS syntax_text
FROM system.help;
ALTER TABLE sq_oracle_manual_syntax ADD CONSTRAINT sq_oracle_manual_syntax_pk
  PRIMARY KEY (syntax_topic, syntax_seq);

CREATE TABLE sq_oracle_manual_statements (
  statement_name VARCHAR2(100) CONSTRAINT sq_oracle_manual_stmt_pk PRIMARY KEY,
  execution_class VARCHAR2(12) DEFAULT 'CATALOG_ONLY' NOT NULL,
  CONSTRAINT sq_oracle_manual_stmt_class_ck
    CHECK (execution_class IN ('LIVE', 'CATALOG_ONLY'))
) TABLESPACE users;

INSERT INTO sq_oracle_manual_statements(statement_name)
SELECT statement_name
FROM JSON_TABLE(
  TO_CLOB(q'~[
    "ADMINISTER KEY MANAGEMENT",
    "ALTER ANALYTIC VIEW", "ALTER ASSERTION", "ALTER ATTRIBUTE DIMENSION",
    "ALTER AUDIT POLICY", "ALTER CLUSTER", "ALTER DATABASE",
    "ALTER DATABASE DICTIONARY", "ALTER DATABASE LINK", "ALTER DIMENSION",
    "ALTER DIRECTIVE (VALIDATE)", "ALTER DISKGROUP", "ALTER DOMAIN",
    "ALTER END USER", "ALTER FLASHBACK ARCHIVE", "ALTER FUNCTION",
    "ALTER HIERARCHY", "ALTER INDEX", "ALTER INDEXTYPE",
    "ALTER INMEMORY JOIN GROUP", "ALTER JAVA",
    "ALTER JSON RELATIONAL DUALITY VIEW", "ALTER LIBRARY",
    "ALTER LOCKDOWN PROFILE", "ALTER MATERIALIZED VIEW",
    "ALTER MATERIALIZED VIEW LOG", "ALTER MATERIALIZED ZONEMAP",
    "ALTER MLE ENV", "ALTER MLE MODULE", "ALTER OPERATOR", "ALTER OUTLINE",
    "ALTER PACKAGE", "ALTER PLUGGABLE DATABASE", "ALTER PMEM FILESTORE",
    "ALTER PROCEDURE", "ALTER PROFILE", "ALTER PROPERTY GRAPH",
    "ALTER RESOURCE COST", "ALTER ROLE", "ALTER ROLLBACK SEGMENT",
    "ALTER SEQUENCE", "ALTER SESSION", "ALTER SYNONYM", "ALTER SYSTEM",
    "ALTER TABLE", "ALTER TABLESPACE", "ALTER TABLESPACE SET",
    "ALTER TRIGGER", "ALTER TYPE", "ALTER USER", "ALTER VIEW", "ANALYZE",
    "ASSOCIATE STATISTICS", "AUDIT (Unified Auditing)", "CALL", "COMMENT",
    "COMMIT",
    "CREATE ANALYTIC VIEW", "CREATE APPLICATION IDENTITY",
    "CREATE ASSERTION", "CREATE ATTRIBUTE DIMENSION", "CREATE AUDIT POLICY",
    "CREATE CLUSTER", "CREATE CONTEXT", "CREATE CONTROLFILE",
    "CREATE DATA GRANT", "CREATE DATA ROLE", "CREATE DATABASE",
    "CREATE DATABASE LINK", "CREATE DIMENSION", "CREATE DIRECTIVE (VALIDATE)",
    "CREATE DIRECTORY", "CREATE DISKGROUP", "CREATE DOMAIN", "CREATE EDITION",
    "CREATE END USER", "CREATE END USER CONTEXT", "CREATE FLASHBACK ARCHIVE",
    "CREATE FLEXIBLE DOMAIN", "CREATE FUNCTION", "CREATE HIERARCHY",
    "CREATE HYBRID VECTOR INDEX", "CREATE ICEBERG TABLE", "CREATE INDEX",
    "CREATE INDEXTYPE", "CREATE INMEMORY JOIN GROUP", "CREATE JAVA",
    "CREATE JSON RELATIONAL DUALITY VIEW", "CREATE LIBRARY",
    "CREATE LOCKDOWN PROFILE", "CREATE LOGICAL PARTITION TRACKING",
    "CREATE MATERIALIZED VIEW", "CREATE MATERIALIZED VIEW LOG",
    "CREATE MATERIALIZED ZONEMAP", "CREATE MLE ENV", "CREATE MLE MODULE",~') ||
  TO_CLOB(q'~
    "CREATE MULTI COLUMN DOMAIN", "CREATE OPERATOR", "CREATE OUTLINE",
    "CREATE PACKAGE", "CREATE PACKAGE BODY", "CREATE PFILE",
    "CREATE PLUGGABLE DATABASE", "CREATE PMEM FILESTORE", "CREATE PROCEDURE",
    "CREATE PROFILE", "CREATE PROPERTY GRAPH", "CREATE RESTORE POINT",
    "CREATE ROLE", "CREATE ROLLBACK SEGMENT", "CREATE SCHEMA",
    "CREATE SEQUENCE", "CREATE SINGLE COLUMN DOMAIN", "CREATE SPFILE",
    "CREATE SYNONYM", "CREATE TABLE", "CREATE TABLESPACE",
    "CREATE TABLESPACE SET", "CREATE TRIGGER", "CREATE TYPE",
    "CREATE TYPE BODY", "CREATE USER", "CREATE VECTOR INDEX", "CREATE VIEW",
    "DELETE", "DISASSOCIATE STATISTICS", "DROP ANALYTIC VIEW",
    "DROP APPLICATION IDENTITY", "DROP ASSERTION", "DROP ATTRIBUTE DIMENSION",
    "DROP AUDIT POLICY", "DROP CLUSTER", "DROP CONTEXT", "DROP DATA GRANT",
    "DROP DATA ROLE", "DROP DATABASE", "DROP DATABASE LINK", "DROP DIMENSION",
    "DROP DIRECTIVE (VALIDATE)", "DROP DIRECTORY", "DROP DISKGROUP",
    "DROP DOMAIN", "DROP EDITION", "DROP END USER", "DROP FLASHBACK ARCHIVE",
    "DROP FUNCTION", "DROP HIERARCHY", "DROP ICEBERG TABLE", "DROP INDEX",
    "DROP INDEXTYPE", "DROP INMEMORY JOIN GROUP", "DROP JAVA", "DROP LIBRARY",
    "DROP LOCKDOWN PROFILE", "DROP MATERIALIZED VIEW",
    "DROP MATERIALIZED VIEW LOG", "DROP MATERIALIZED ZONEMAP", "DROP MLE ENV",
    "DROP MLE MODULE", "DROP OPERATOR", "DROP OUTLINE", "DROP PACKAGE",
    "DROP PLUGGABLE DATABASE", "DROP PMEM FILESTORE", "DROP PROCEDURE",
    "DROP PROFILE", "DROP PROPERTY GRAPH", "DROP RESTORE POINT", "DROP ROLE",
    "DROP ROLLBACK SEGMENT", "DROP SEQUENCE", "DROP SYNONYM", "DROP TABLE",
    "DROP TABLESPACE", "DROP TABLESPACE SET", "DROP TRIGGER", "DROP TYPE",
    "DROP TYPE BODY", "DROP USER", "DROP VIEW", "EXPLAIN PLAN",
    "FLASHBACK DATABASE", "FLASHBACK TABLE", "GRANT", "GRANT DATA ROLE",
    "INSERT", "LOCK TABLE", "MERGE", "NOAUDIT (Traditional Auditing)",
    "NOAUDIT (Unified Auditing)", "PURGE", "RENAME", "REVOKE",
    "REVOKE DATA ROLE", "ROLLBACK", "SAVEPOINT", "SELECT",
    "SET CONSTRAINT[S]", "SET ROLE", "SET TRANSACTION",
    "SET USE DATA GRANTS ONLY", "TRUNCATE CLUSTER", "TRUNCATE TABLE", "UPDATE"
  ]~'),
  '$[*]' COLUMNS(statement_name VARCHAR2(100) PATH '$')
);

UPDATE sq_oracle_manual_statements
SET execution_class = 'LIVE'
WHERE statement_name IN (
  'ALTER INDEX', 'ALTER MATERIALIZED VIEW', 'ALTER PROPERTY GRAPH',
  'ALTER SEQUENCE', 'ALTER SESSION', 'ALTER SYNONYM', 'ALTER TABLE',
  'ANALYZE', 'CALL', 'COMMENT', 'COMMIT', 'CREATE DOMAIN', 'CREATE FUNCTION',
  'CREATE INDEX', 'CREATE MATERIALIZED VIEW', 'CREATE MATERIALIZED VIEW LOG',
  'CREATE PACKAGE', 'CREATE PACKAGE BODY', 'CREATE PROCEDURE',
  'CREATE PROPERTY GRAPH', 'CREATE SEQUENCE', 'CREATE SYNONYM', 'CREATE TABLE',
  'CREATE TRIGGER', 'CREATE TYPE', 'CREATE TYPE BODY', 'CREATE VIEW', 'DELETE',
  'DROP DOMAIN', 'DROP FUNCTION', 'DROP INDEX', 'DROP MATERIALIZED VIEW',
  'DROP MATERIALIZED VIEW LOG', 'DROP PACKAGE', 'DROP PROCEDURE',
  'DROP PROPERTY GRAPH', 'DROP SEQUENCE', 'DROP SYNONYM', 'DROP TABLE',
  'DROP TRIGGER', 'DROP TYPE', 'DROP VIEW', 'EXPLAIN PLAN', 'INSERT',
  'LOCK TABLE', 'MERGE', 'ROLLBACK', 'SAVEPOINT', 'SELECT', 'SET TRANSACTION',
  'TRUNCATE TABLE', 'UPDATE'
);

CREATE TABLE sq_oracle_manual_log (
  id NUMBER DEFAULT sq_oracle_manual_seq.NEXTVAL,
  category VARCHAR2(32) DOMAIN sq_oracle_category_domain NOT NULL,
  payload JSON,
  amount NUMBER(12,2) DEFAULT 0 NOT NULL,
  created_at TIMESTAMP(6) DEFAULT SYSTIMESTAMP NOT NULL,
  payload_kind VARCHAR2(32)
    GENERATED ALWAYS AS (JSON_VALUE(payload, '$.kind' RETURNING VARCHAR2(32))) VIRTUAL,
  CONSTRAINT sq_oracle_manual_pk PRIMARY KEY (id),
  CONSTRAINT sq_oracle_manual_uk UNIQUE (category),
  CONSTRAINT sq_oracle_manual_amount_ck CHECK (amount >= 0)
) TABLESPACE users;

CREATE TABLE sq_oracle_manual_node (
  node_id NUMBER CONSTRAINT sq_oracle_manual_node_pk PRIMARY KEY,
  parent_node_id NUMBER,
  node_name VARCHAR2(80) NOT NULL,
  attributes JSON NOT NULL,
  CONSTRAINT sq_oracle_manual_node_fk
    FOREIGN KEY (parent_node_id) REFERENCES sq_oracle_manual_node(node_id)
) TABLESPACE users;

CREATE TABLE sq_oracle_manual_edge (
  edge_id NUMBER CONSTRAINT sq_oracle_manual_edge_pk PRIMARY KEY,
  source_node_id NUMBER NOT NULL,
  target_node_id NUMBER NOT NULL,
  edge_label VARCHAR2(32) NOT NULL,
  edge_weight NUMBER(10,2) DEFAULT 1 NOT NULL,
  CONSTRAINT sq_oracle_manual_edge_source_fk
    FOREIGN KEY (source_node_id) REFERENCES sq_oracle_manual_node(node_id),
  CONSTRAINT sq_oracle_manual_edge_target_fk
    FOREIGN KEY (target_node_id) REFERENCES sq_oracle_manual_node(node_id)
) TABLESPACE users;

CREATE TABLE sq_oracle_manual_metric (
  metric_id NUMBER NOT NULL,
  node_id NUMBER NOT NULL,
  metric_day DATE NOT NULL,
  metric_name VARCHAR2(32) NOT NULL,
  metric_value NUMBER(12,2) NOT NULL,
  payload JSON,
  CONSTRAINT sq_oracle_manual_metric_pk PRIMARY KEY (metric_id, metric_day)
) TABLESPACE users
PARTITION BY RANGE (metric_day) (
  PARTITION p2025 VALUES LESS THAN (DATE '2026-01-01'),
  PARTITION p2026 VALUES LESS THAN (DATE '2027-01-01'),
  PARTITION pmax VALUES LESS THAN (MAXVALUE)
);

CREATE TABLE sq_oracle_manual_lob (
  lob_id NUMBER CONSTRAINT sq_oracle_manual_lob_pk PRIMARY KEY,
  text_doc CLOB,
  national_doc NCLOB,
  binary_doc BLOB,
  xml_doc XMLTYPE
) TABLESPACE users
LOB (text_doc) STORE AS SECUREFILE
LOB (national_doc) STORE AS SECUREFILE
LOB (binary_doc) STORE AS SECUREFILE;

CREATE GLOBAL TEMPORARY TABLE sq_oracle_manual_gtt (
  id NUMBER,
  value_text VARCHAR2(100)
) ON COMMIT DELETE ROWS;
ALTER TABLE sq_oracle_manual_gtt ADD (created_at TIMESTAMP DEFAULT SYSTIMESTAMP);
ALTER TABLE sq_oracle_manual_gtt DROP COLUMN created_at;
TRUNCATE TABLE sq_oracle_manual_gtt;

CREATE TABLE sq_oracle_manual_child (
  child_id NUMBER GENERATED BY DEFAULT ON NULL AS IDENTITY,
  log_id NUMBER NOT NULL,
  note_text VARCHAR2(200),
  embedding VECTOR(3, FLOAT32),
  CONSTRAINT sq_oracle_manual_child_pk PRIMARY KEY (child_id),
  CONSTRAINT sq_oracle_manual_child_fk
    FOREIGN KEY (log_id) REFERENCES sq_oracle_manual_log(id)
) TABLESPACE users;

CREATE INDEX sq_oracle_manual_kind_ix
  ON sq_oracle_manual_log(payload_kind, amount DESC) INVISIBLE;
ALTER INDEX sq_oracle_manual_kind_ix VISIBLE;
ALTER INDEX sq_oracle_manual_kind_ix MONITORING USAGE;

COMMENT ON TABLE sq_oracle_manual_log IS 'Oracle final syntax certification';
COMMENT ON COLUMN sq_oracle_manual_log.category IS 'statement category';

CREATE OR REPLACE TYPE sq_oracle_manual_obj AS OBJECT (
  amount NUMBER,
  MEMBER FUNCTION band RETURN VARCHAR2
);
/
CREATE OR REPLACE TYPE BODY sq_oracle_manual_obj AS
  MEMBER FUNCTION band RETURN VARCHAR2 IS
  BEGIN
    RETURN CASE WHEN amount >= 50 THEN 'HIGH' ELSE 'LOW' END;
  END;
END;
/

CREATE OR REPLACE FUNCTION sq_oracle_manual_fn(p_amount NUMBER)
RETURN VARCHAR2
DETERMINISTIC
IS
BEGIN
  RETURN sq_oracle_manual_obj(p_amount).band();
END;
/

CREATE OR REPLACE PROCEDURE sq_oracle_manual_proc(
  p_category IN VARCHAR2,
  p_amount IN NUMBER DEFAULT 0
)
AUTHID DEFINER
IS
BEGIN
  INSERT INTO sq_oracle_manual_log(category, payload, amount)
  VALUES (p_category, JSON_OBJECT('kind' VALUE LOWER(p_category)), p_amount);
END;
/

CREATE OR REPLACE PACKAGE sq_oracle_manual_pkg AUTHID DEFINER AS
  PROCEDURE assert_true(p_condition BOOLEAN, p_message VARCHAR2);
  PROCEDURE run_compound;
  FUNCTION row_count RETURN PLS_INTEGER;
END sq_oracle_manual_pkg;
/
CREATE OR REPLACE PACKAGE BODY sq_oracle_manual_pkg AS
  PROCEDURE assert_true(p_condition BOOLEAN, p_message VARCHAR2) IS
  BEGIN
    IF p_condition IS NULL OR NOT p_condition THEN
      RAISE_APPLICATION_ERROR(-20000, p_message);
    END IF;
  END;

  PROCEDURE run_compound IS
    TYPE number_list IS TABLE OF NUMBER;
    amount_values number_list;
    total_amount NUMBER := 0;
    loop_count PLS_INTEGER := 0;
    CURSOR amount_cursor IS
      SELECT amount FROM sq_oracle_manual_log ORDER BY id;

    PROCEDURE exercise_goto(p_limit PLS_INTEGER) IS
      counter_value PLS_INTEGER := 0;
    BEGIN
      <<again>>
      counter_value := counter_value + 1;
      IF counter_value < p_limit THEN
        GOTO again;
      END IF;
    END;
  BEGIN
    SELECT amount
    BULK COLLECT INTO amount_values
    FROM sq_oracle_manual_log
    ORDER BY id;

    FORALL index_value IN INDICES OF amount_values SAVE EXCEPTIONS
      UPDATE sq_oracle_manual_log
      SET amount = amount
      WHERE id = -amount_values(index_value);

    FOR amount_row IN amount_cursor LOOP
      total_amount := total_amount + amount_row.amount;
      loop_count := loop_count + 1;
      EXIT WHEN loop_count > 100;
    END LOOP;

    CASE
      WHEN total_amount > 0 THEN NULL;
      ELSE RAISE_APPLICATION_ERROR(-20001, 'compound total');
    END CASE;
    exercise_goto(2);
  EXCEPTION
    WHEN OTHERS THEN
      RAISE;
  END;

  FUNCTION row_count RETURN PLS_INTEGER IS
    result_value PLS_INTEGER;
  BEGIN
    SELECT COUNT(*) INTO result_value FROM sq_oracle_manual_log;
    RETURN result_value;
  END;
END sq_oracle_manual_pkg;
/

CREATE OR REPLACE TRIGGER sq_oracle_manual_biu
BEFORE INSERT OR UPDATE OF category, amount ON sq_oracle_manual_log
FOR EACH ROW
BEGIN
  :NEW.category := UPPER(TRIM(:NEW.category));
  :NEW.amount := ROUND(:NEW.amount, 2);
END;
/

CALL sq_oracle_manual_proc('ddl', 10);
EXECUTE sq_oracle_manual_proc('dml', 20)
BEGIN
  sq_oracle_manual_proc('query', 30);
END;
/

INSERT INTO sq_oracle_manual_node(node_id, parent_node_id, node_name, attributes) VALUES
  (1, NULL, 'Root', JSON_OBJECT('tier' VALUE 'root'));
INSERT INTO sq_oracle_manual_node VALUES
  (2, 1, 'North', JSON_OBJECT('tier' VALUE 'branch'));
INSERT INTO sq_oracle_manual_node VALUES
  (3, 1, 'South', JSON_OBJECT('tier' VALUE 'branch'));
INSERT INTO sq_oracle_manual_node VALUES
  (4, 2, 'Leaf', JSON_OBJECT('tier' VALUE 'leaf'));

INSERT INTO sq_oracle_manual_edge VALUES (1, 1, 2, 'OWNS', 1);
INSERT INTO sq_oracle_manual_edge VALUES (2, 1, 3, 'OWNS', 1);
INSERT INTO sq_oracle_manual_edge VALUES (3, 2, 4, 'OWNS', 2);

INSERT INTO sq_oracle_manual_metric VALUES
  (1, 1, DATE '2025-12-31', 'LATENCY', 12,
   JSON_OBJECT('tags' VALUE JSON_ARRAY('sql', 'manual')));
INSERT INTO sq_oracle_manual_metric VALUES
  (2, 1, DATE '2026-01-01', 'LATENCY', 10,
   JSON_OBJECT('tags' VALUE JSON_ARRAY('query')));
INSERT INTO sq_oracle_manual_metric VALUES
  (3, 2, DATE '2026-01-02', 'LATENCY', 20,
   JSON_OBJECT('tags' VALUE JSON_ARRAY('json', 'manual')));
INSERT INTO sq_oracle_manual_metric VALUES
  (4, 2, DATE '2026-01-03', 'ERRORS', 2,
   JSON_OBJECT('tags' VALUE JSON_ARRAY('routine')));
INSERT INTO sq_oracle_manual_metric VALUES
  (5, 3, DATE '2027-01-01', 'LATENCY', 30,
   JSON_OBJECT('tags' VALUE JSON_ARRAY('vector')));

INSERT INTO sq_oracle_manual_lob(
  lob_id, text_doc, national_doc, binary_doc, xml_doc
) VALUES (
  1,
  TO_CLOB('standalone Oracle LOB syntax'),
  TO_NCLOB('national text'),
  EMPTY_BLOB(),
  XMLTYPE('<manual><section name="sql">26ai</section></manual>')
);

INSERT ALL
  INTO sq_oracle_manual_log(id, category, payload, amount)
    VALUES (1000, 'INSERT_ALL_A', JSON_OBJECT('kind' VALUE 'multi'), 40)
  INTO sq_oracle_manual_log(id, category, payload, amount)
    VALUES (1001, 'INSERT_ALL_B', JSON_OBJECT('kind' VALUE 'multi'), 50)
SELECT 1 FROM dual;

INSERT FIRST
  WHEN value_amount < 100 THEN
    INTO sq_oracle_manual_log(category, payload, amount)
    VALUES ('INSERT_FIRST', JSON_OBJECT('kind' VALUE 'conditional'), value_amount)
  ELSE
    INTO sq_oracle_manual_log(category, payload, amount)
    VALUES ('INSERT_ELSE', JSON_OBJECT('kind' VALUE 'conditional'), value_amount)
SELECT 60 value_amount FROM dual;

MERGE INTO sq_oracle_manual_log target
USING (
  SELECT 'DML' category, 25 amount FROM dual UNION ALL
  SELECT 'MERGE_INSERT', 70 FROM dual
) source
ON (target.category = source.category)
WHEN MATCHED THEN UPDATE SET
  target.amount = source.amount,
  target.payload = JSON_OBJECT('kind' VALUE 'merge-update')
WHEN NOT MATCHED THEN INSERT (category, payload, amount)
VALUES (source.category, JSON_OBJECT('kind' VALUE 'merge-insert'), source.amount);

CALL sq_oracle_manual_pkg.run_compound();

INSERT INTO sq_oracle_manual_child(log_id, note_text, embedding)
SELECT id, 'vector child', TO_VECTOR('[1,2,3]')
FROM sq_oracle_manual_log
WHERE category = 'DDL';

CREATE OR REPLACE VIEW sq_oracle_manual_v AS
SELECT id, category, payload_kind, amount, sq_oracle_manual_fn(amount) amount_band
FROM sq_oracle_manual_log
WHERE amount >= 10
WITH READ ONLY;

CREATE SYNONYM sq_oracle_manual_syn FOR sq_oracle_manual_v;
ALTER SYNONYM sq_oracle_manual_syn COMPILE;

CREATE PROPERTY GRAPH sq_oracle_manual_graph
  VERTEX TABLES (
    sq_oracle_manual_node
      KEY (node_id)
      LABEL node PROPERTIES (node_id, node_name)
  )
  EDGE TABLES (
    sq_oracle_manual_edge
      KEY (edge_id)
      SOURCE KEY (source_node_id) REFERENCES sq_oracle_manual_node (node_id)
      DESTINATION KEY (target_node_id) REFERENCES sq_oracle_manual_node (node_id)
      LABEL owns PROPERTIES (edge_label, edge_weight)
  );
ALTER PROPERTY GRAPH sq_oracle_manual_graph COMPILE;

CREATE MATERIALIZED VIEW LOG ON sq_oracle_manual_log
  WITH PRIMARY KEY, ROWID (category, amount)
  INCLUDING NEW VALUES;
CREATE MATERIALIZED VIEW sq_oracle_manual_mv
  BUILD IMMEDIATE
  REFRESH COMPLETE ON DEMAND
  AS SELECT category, SUM(amount) total_amount
     FROM sq_oracle_manual_log
     GROUP BY category;
ALTER MATERIALIZED VIEW sq_oracle_manual_mv COMPILE;

SAVEPOINT before_rollback;
UPDATE sq_oracle_manual_log SET amount = amount + 1 WHERE category = 'DDL';
DELETE FROM sq_oracle_manual_log WHERE category = 'NEVER_MATCHES';
ROLLBACK TO before_rollback;
COMMIT WRITE IMMEDIATE WAIT;

SET TRANSACTION READ WRITE NAME 'sq-oracle-final';
LOCK TABLE sq_oracle_manual_log IN ROW SHARE MODE NOWAIT;
COMMIT;

DECLARE
  returned_amount NUMBER;
BEGIN
  UPDATE sq_oracle_manual_log
  SET amount = amount
  WHERE category = 'DDL'
  RETURNING amount INTO returned_amount;
  sq_oracle_manual_pkg.assert_true(returned_amount = 10, 'RETURNING');
END;
/

DECLARE
  dynamic_count PLS_INTEGER;
BEGIN
  EXECUTE IMMEDIATE
    'SELECT COUNT(*) FROM sq_oracle_manual_log WHERE amount >= :1'
    INTO dynamic_count USING 10;
  sq_oracle_manual_pkg.assert_true(dynamic_count = 7, 'dynamic SQL');
END;
/

EXPLAIN PLAN SET STATEMENT_ID = 'SQ_ORACLE_FINAL' FOR
SELECT category, SUM(amount)
FROM sq_oracle_manual_log
GROUP BY category;
SELECT operation, options, object_name
FROM plan_table
WHERE statement_id = 'SQ_ORACLE_FINAL'
ORDER BY id;

ANALYZE TABLE sq_oracle_manual_log
  COMPUTE STATISTICS FOR TABLE FOR ALL INDEXED COLUMNS;
UPDATE sq_oracle_manual_log SET amount = amount WHERE id = -1;
COMMIT;

WITH node_tree(node_id, parent_node_id, node_name, depth_no, node_path) AS (
  SELECT node_id, parent_node_id, node_name, 0,
         CAST('/' || node_name AS VARCHAR2(400))
  FROM sq_oracle_manual_node
  WHERE parent_node_id IS NULL
  UNION ALL
  SELECT n.node_id, n.parent_node_id, n.node_name, t.depth_no + 1,
         t.node_path || '/' || n.node_name
  FROM sq_oracle_manual_node n
  JOIN node_tree t ON t.node_id = n.parent_node_id
)
SEARCH DEPTH FIRST BY node_name SET traversal_order
CYCLE node_id SET cycle_mark TO 'Y' DEFAULT 'N'
SELECT node_id, parent_node_id, node_name, depth_no, node_path, cycle_mark
FROM node_tree
ORDER BY traversal_order;

SELECT node_id, parent_node_id, node_name,
       LEVEL depth_no,
       CONNECT_BY_ISLEAF leaf_flag,
       SYS_CONNECT_BY_PATH(node_name, '/') node_path
FROM sq_oracle_manual_node
START WITH parent_node_id IS NULL
CONNECT BY NOCYCLE PRIOR node_id = parent_node_id
ORDER SIBLINGS BY node_name;

SELECT m.metric_id, m.metric_name, tags.tag_no, tags.tag_value
FROM sq_oracle_manual_metric m
CROSS APPLY JSON_TABLE(
  m.payload,
  '$.tags[*]' COLUMNS (
    tag_no FOR ORDINALITY,
    tag_value VARCHAR2(32) PATH '$'
  )
) tags
ORDER BY m.metric_id, tags.tag_no;

SELECT x.section_name, x.section_value
FROM sq_oracle_manual_lob l
CROSS JOIN XMLTABLE(
  '/manual/section'
  PASSING l.xml_doc
  COLUMNS
    section_name VARCHAR2(32) PATH '@name',
    section_value VARCHAR2(32) PATH 'text()'
) x;

SELECT node_id, metric_day, metric_name, metric_value,
       SUM(metric_value) OVER (
         PARTITION BY node_id
         ORDER BY metric_day, metric_id
         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
       ) running_value,
       LAG(metric_value, 1, 0) OVER (
         PARTITION BY node_id ORDER BY metric_day, metric_id
       ) previous_value,
       DENSE_RANK() OVER (
         PARTITION BY metric_name ORDER BY metric_value DESC
       ) value_rank
FROM sq_oracle_manual_metric
ORDER BY node_id, metric_day;

SELECT node_id, metric_name, SUM(metric_value) total_value,
       GROUPING_ID(node_id, metric_name) grouping_value
FROM sq_oracle_manual_metric
GROUP BY GROUPING SETS ((node_id, metric_name), (node_id), ());

SELECT *
FROM (
  SELECT node_id, metric_name, metric_value
  FROM sq_oracle_manual_metric
)
PIVOT (
  SUM(metric_value) FOR metric_name IN (
    'LATENCY' AS latency,
    'ERRORS' AS errors
  )
)
ORDER BY node_id;

SELECT node_id, metric_name, metric_value
FROM (
  SELECT 1 node_id, 10 latency, 2 errors FROM dual
)
UNPIVOT INCLUDE NULLS (
  metric_value FOR metric_name IN (
    latency AS 'LATENCY',
    errors AS 'ERRORS'
  )
);

SELECT node_id, match_no, first_day, last_day
FROM sq_oracle_manual_metric
MATCH_RECOGNIZE (
  PARTITION BY node_id
  ORDER BY metric_day
  MEASURES
    MATCH_NUMBER() AS match_no,
    FIRST(start_row.metric_day) AS first_day,
    LAST(rise_row.metric_day) AS last_day
  ONE ROW PER MATCH
  AFTER MATCH SKIP PAST LAST ROW
  PATTERN (start_row rise_row*)
  DEFINE rise_row AS rise_row.metric_value >= PREV(rise_row.metric_value)
)
ORDER BY node_id, match_no;

SELECT n.node_id, n.node_name, top_metric.metric_name, top_metric.metric_value
FROM sq_oracle_manual_node n
OUTER APPLY (
  SELECT metric_name, metric_value
  FROM sq_oracle_manual_metric m
  WHERE m.node_id = n.node_id
  ORDER BY metric_value DESC, metric_id
  FETCH FIRST 1 ROW ONLY
) top_metric
ORDER BY n.node_id;

WITH
FUNCTION classify_amount(p_amount NUMBER) RETURN VARCHAR2 IS
BEGIN
  RETURN CASE WHEN p_amount >= 50 THEN 'HIGH' ELSE 'LOW' END;
END;
SELECT category, classify_amount(amount) amount_class
FROM sq_oracle_manual_log
ORDER BY id;
/

(SELECT node_id FROM sq_oracle_manual_node WHERE parent_node_id IS NOT NULL)
INTERSECT
(SELECT node_id FROM sq_oracle_manual_metric WHERE metric_value >= 10)
MINUS
(SELECT node_id FROM sq_oracle_manual_node WHERE node_name = 'Never')
ORDER BY node_id;

SELECT l.category,
       l.amount,
       JSON_SERIALIZE(l.payload RETURNING VARCHAR2 PRETTY) payload_text,
       c.note_text,
       VECTOR_DISTANCE(c.embedding, TO_VECTOR('[1,1,1]'), COSINE) vector_distance
FROM sq_oracle_manual_log l
LEFT OUTER JOIN sq_oracle_manual_child c ON c.log_id = l.id
ORDER BY l.id
FETCH FIRST 10 ROWS ONLY;

VARIABLE sq_oracle_flashback_scn NUMBER
BEGIN
  :sq_oracle_flashback_scn := DBMS_FLASHBACK.GET_SYSTEM_CHANGE_NUMBER;
END;
/
SELECT COUNT(*) flashback_help_rows
FROM system.help AS OF SCN :sq_oracle_flashback_scn;

DECLARE
  log_count PLS_INTEGER;
  ddl_amount NUMBER;
  node_count PLS_INTEGER;
  metric_count PLS_INTEGER;
  graph_count PLS_INTEGER;
  keyword_count PLS_INTEGER;
  source_keyword_count PLS_INTEGER;
  statement_count PLS_INTEGER;
  syntax_count PLS_INTEGER;
  source_syntax_count PLS_INTEGER;
  manual_topic_count PLS_INTEGER;
BEGIN
  SELECT COUNT(*) INTO log_count FROM sq_oracle_manual_log;
  SELECT amount INTO ddl_amount
  FROM sq_oracle_manual_log
  WHERE category = 'DDL';
  SELECT COUNT(*) INTO node_count FROM sq_oracle_manual_node;
  SELECT COUNT(*) INTO metric_count FROM sq_oracle_manual_metric;
  SELECT COUNT(*) INTO graph_count
  FROM user_property_graphs
  WHERE graph_name = 'SQ_ORACLE_MANUAL_GRAPH';
  SELECT COUNT(*) INTO keyword_count FROM sq_oracle_manual_keywords;
  SELECT COUNT(*) INTO source_keyword_count FROM v$reserved_words;
  SELECT COUNT(*) INTO statement_count FROM sq_oracle_manual_statements;
  SELECT COUNT(*) INTO syntax_count FROM sq_oracle_manual_syntax;
  SELECT COUNT(*) INTO source_syntax_count FROM system.help;
  SELECT COUNT(*) INTO manual_topic_count FROM sq_oracle_manual_topics;

  sq_oracle_manual_pkg.assert_true(
    log_count = 7 AND sq_oracle_manual_pkg.row_count = 7,
    'manual statement row count'
  );
  sq_oracle_manual_pkg.assert_true(
    ddl_amount = 10,
    'rollback'
  );
  sq_oracle_manual_pkg.assert_true(
    node_count = 4,
    'node data'
  );
  sq_oracle_manual_pkg.assert_true(
    metric_count = 5,
    'partition data'
  );
  sq_oracle_manual_pkg.assert_true(
    graph_count = 1,
    'property graph'
  );
  sq_oracle_manual_pkg.assert_true(
    keyword_count = source_keyword_count,
    'official keyword coverage'
  );
  sq_oracle_manual_pkg.assert_true(
    statement_count = 204,
    'official statement-family coverage'
  );
  sq_oracle_manual_pkg.assert_true(
    syntax_count = source_syntax_count,
    'official installed syntax-topic coverage'
  );
  sq_oracle_manual_pkg.assert_true(
    manual_topic_count = 1073,
    'official manual topic coverage'
  );
END;
/

SELECT 'PASS' final_status,
       banner_full server_version,
       (SELECT COUNT(*) FROM sq_oracle_manual_log) manual_rows,
       (SELECT COUNT(*) FROM sq_oracle_manual_keywords) keyword_count,
       (SELECT COUNT(*) FROM sq_oracle_manual_statements) statement_family_count,
       (SELECT COUNT(*) FROM sq_oracle_manual_syntax) syntax_topic_count,
       (SELECT COUNT(*) FROM sq_oracle_manual_topics) manual_topic_count
FROM v$version
WHERE banner_full LIKE 'Oracle%';

PROMPT [ORACLE FINAL] PASS
