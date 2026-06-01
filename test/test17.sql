/* 
    EXECUTION UNIT SPLITTER - FINAL BOSS
    아래 텍스트들은 진짜 종료가 아니다.
    END;
    /
    CREATE OR REPLACE PACKAGE fake_pkg IS
        PROCEDURE p;
    END;
    /
*/

--------------------------------------------------------------------------------
-- UNIT 01 : CLEANUP ANONYMOUS BLOCK
--------------------------------------------------------------------------------
DECLARE
    PROCEDURE drop_if_exists(p_sql VARCHAR2) IS
    BEGIN
        EXECUTE IMMEDIATE p_sql;
    EXCEPTION
        WHEN OTHERS THEN
            NULL;
    END drop_if_exists;
BEGIN
    drop_if_exists('drop trigger qt_split_trg');
    drop_if_exists('drop view qt_split_view');
    drop_if_exists('drop procedure qt_split_proc');
    drop_if_exists('drop function qt_split_fun');
    drop_if_exists('drop package qt_split_pkg');
    drop_if_exists('drop type qt_split_obj force');
    drop_if_exists('drop sequence qt_split_seq');
    drop_if_exists('drop table qt_split_log purge');
    drop_if_exists('drop table qt_split_data purge');
END;
/

--------------------------------------------------------------------------------
-- UNIT 02 : CREATE LOG TABLE
--------------------------------------------------------------------------------
CREATE TABLE qt_split_log
(
    log_id     NUMBER PRIMARY KEY,
    unit_name  VARCHAR2(128),
    payload    VARCHAR2(4000),
    created_at TIMESTAMP DEFAULT SYSTIMESTAMP NOT NULL
);

--------------------------------------------------------------------------------
-- UNIT 03 : CREATE DATA TABLE
--------------------------------------------------------------------------------
CREATE TABLE qt_split_data
(
    id         NUMBER PRIMARY KEY,
    grp_code   VARCHAR2(30) NOT NULL,
    txt        VARCHAR2(400),
    amt        NUMBER(18,4),
    note       VARCHAR2(4000),
    created_at DATE DEFAULT SYSDATE NOT NULL
);

--------------------------------------------------------------------------------
-- UNIT 04 : CREATE SEQUENCE
--------------------------------------------------------------------------------
CREATE SEQUENCE qt_split_seq
    START WITH 1
    INCREMENT BY 1
    NOCACHE;

/*
    fake boundary noise block comment
    ;
    /
    BEGIN
      NULL;
    END;
    /
*/

--------------------------------------------------------------------------------
-- UNIT 05 : INSERT ALL
--------------------------------------------------------------------------------
INSERT ALL
    INTO qt_split_data (id, grp_code, txt, amt, note)
    VALUES (1, 'G/A', q'[alpha ; one / not terminator]', 10.5, q'{seed-1 ; / -- /* */}')
    INTO qt_split_data (id, grp_code, txt, amt, note)
    VALUES (2, 'G/B', q'{beta ''two'' ; still text}', -20.25, q'<seed-2 with BEGIN NULL; END; />')
    INTO qt_split_data (id, grp_code, txt, amt, note)
    VALUES (3, 'G/C', 'gamma / delta ; epsilon', 0, 'seed-3 // slash slash ; ;')
SELECT 1
FROM dual;

--------------------------------------------------------------------------------
-- UNIT 06 : MERGE WITH CTE
--------------------------------------------------------------------------------
MERGE INTO qt_split_data d
USING
(
    WITH src AS
    (
        SELECT 2 AS id,
               'G/B' AS grp_code,
               q'[merge ; update / text]' AS txt,
               200 AS amt,
               q'{merged-note ; / ok}' AS note
        FROM dual
        UNION ALL
        SELECT 4,
               'G/D',
               q'(inserted from MERGE ; with / and --)',
               40,
               q'[new row /* not a comment here */]'
        FROM dual
    )
    SELECT *
    FROM src
) s
ON (d.id = s.id)
WHEN MATCHED THEN
    UPDATE
       SET d.txt  = s.txt,
           d.amt  = s.amt / 2,
           d.note = d.note || ' | ' || s.note
WHEN NOT MATCHED THEN
    INSERT (id, grp_code, txt, amt, note)
    VALUES (s.id, s.grp_code, s.txt, s.amt, s.note);

--------------------------------------------------------------------------------
-- UNIT 07 : TYPE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_split_obj AS OBJECT
(
    id       NUMBER,
    grp_code VARCHAR2(30),
    txt      VARCHAR2(400),
    amt      NUMBER,
    MEMBER FUNCTION render RETURN VARCHAR2
);
/

--------------------------------------------------------------------------------
-- UNIT 08 : TYPE BODY
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE BODY qt_split_obj AS
    MEMBER FUNCTION render RETURN VARCHAR2 IS
    BEGIN
        RETURN '[' || grp_code || '] ' || txt || ' = ' || TO_CHAR(amt) || q'[ ; rendered / ok ]';
    END render;
END;
/

--------------------------------------------------------------------------------
-- UNIT 09 : PACKAGE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_split_pkg AUTHID DEFINER IS
    SUBTYPE t_small_text IS VARCHAR2(100);

    c_magic CONSTANT VARCHAR2(100) := q'[pkg-const ; / -- /* keep */ ]';

    PROCEDURE log_msg(p_unit_name VARCHAR2, p_payload VARCHAR2);
    FUNCTION make_note(p_id NUMBER, p_text VARCHAR2) RETURN VARCHAR2;
    FUNCTION build_obj(p_id NUMBER, p_grp VARCHAR2, p_txt VARCHAR2, p_amt NUMBER) RETURN qt_split_obj;
    PROCEDURE seed_more(p_base NUMBER);
END qt_split_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 10 : PACKAGE BODY
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_split_pkg IS

    PROCEDURE log_msg(p_unit_name VARCHAR2, p_payload VARCHAR2) IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO qt_split_log (log_id, unit_name, payload)
        VALUES (qt_split_seq.NEXTVAL, p_unit_name, SUBSTR(p_payload, 1, 4000));

        COMMIT;
    END log_msg;

    FUNCTION make_note(p_id NUMBER, p_text VARCHAR2) RETURN VARCHAR2 IS
        v_note  VARCHAR2(4000);
        v_regex CONSTANT VARCHAR2(100) := '^\s*/\s*$';
    BEGIN
        v_note :=
               'ID=' || p_id
            || ' | TXT=' || REPLACE(p_text, CHR(10), '\n')
            || ' | REGEX=' || v_regex
            || q'[ | q1=; / -- ]'
            || q'{ | q2=/* not comment */ }';

        RETURN v_note;
    END make_note;

    FUNCTION build_obj(p_id NUMBER, p_grp VARCHAR2, p_txt VARCHAR2, p_amt NUMBER) RETURN qt_split_obj IS
    BEGIN
        RETURN qt_split_obj(p_id, p_grp, p_txt, p_amt);
    END build_obj;

    PROCEDURE seed_more(p_base NUMBER) IS
        v_sql        VARCHAR2(32767);
        v_plsql      VARCHAR2(32767);
        v_payload    VARCHAR2(4000);
        v_div_result NUMBER;

        PROCEDURE add_one(p_offset NUMBER, p_grp VARCHAR2, p_txt VARCHAR2, p_amt NUMBER) IS
        BEGIN
            v_sql := q'[
                INSERT INTO qt_split_data(id, grp_code, txt, amt, note)
                VALUES (:1, :2, :3, :4, :5)
            ]';

            EXECUTE IMMEDIATE v_sql
                USING p_base + p_offset,
                      p_grp,
                      p_txt,
                      p_amt,
                      make_note(p_base + p_offset, p_txt);
        END add_one;
    BEGIN
        v_div_result := 12 / 3;

        add_one(1, 'G/X', q'[x-row ; / ; --]', 11);
        add_one(2, 'G/Y', q'{y-row ''quoted'' ; /* text */}', 22);
        add_one(3, 'G/Z', q'<z-row BEGIN NULL; END; />', 33);

        v_plsql := q'[
            BEGIN
                qt_split_pkg.log_msg(:u, :p);
            END;
        ]';

        v_payload := q'[seed_more payload:
/
BEGIN
  NULL; -- this slash is text, not terminator
END;
/]' || ' | div=' || v_div_result;

        EXECUTE IMMEDIATE v_plsql USING 'PKG.SEED_MORE', v_payload;

        /*
            fake terminators inside comment
            END;
            /
            CREATE OR REPLACE PACKAGE whatever;
        */
    END seed_more;

END qt_split_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 11 : STANDALONE FUNCTION
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION qt_split_fun
(
    p_prefix VARCHAR2 DEFAULT q'<FUN;PREFIX/OK>'
)
RETURN VARCHAR2
IS
    v_text VARCHAR2(4000);
BEGIN
    v_text := p_prefix || ' -> ' || qt_split_pkg.make_note(900, q'[fun-body ; / -- /* */]');

    qt_split_pkg.log_msg('FUNCTION', v_text);

    RETURN SUBSTR(v_text, 1, 4000);
END qt_split_fun;
/

--------------------------------------------------------------------------------
-- UNIT 12 : STANDALONE PROCEDURE
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_split_proc(p_multiplier NUMBER)
IS
    CURSOR c_data IS
        SELECT id, grp_code, NVL(amt, 0) AS amt
        FROM qt_split_data
        WHERE id <= 10
        ORDER BY id;

    v_sum NUMBER := 0;
BEGIN
    <<outer_loop>>
    FOR r IN c_data LOOP
        BEGIN
            v_sum := v_sum + (r.amt * p_multiplier);

            UPDATE qt_split_data
               SET amt  = NVL(amt, 0)
                          + CASE
                                WHEN MOD(r.id, 2) = 0 THEN p_multiplier
                                ELSE p_multiplier / 2
                            END,
                   note = SUBSTR(NVL(note, '') || q'[ | PROC; / ; touched ]', 1, 4000)
             WHERE id = r.id;

            IF r.id = 3 THEN
                NULL;
            ELSIF r.id = 4 THEN
                NULL;
            ELSE
                NULL;
            END IF;
        EXCEPTION
            WHEN OTHERS THEN
                qt_split_pkg.log_msg('PROC.ERROR', SQLERRM);
        END;
    END LOOP outer_loop;

    qt_split_pkg.log_msg('PROCEDURE', 'sum=' || TO_CHAR(v_sum) || ' | fn=' || qt_split_fun);
END qt_split_proc;
/

--------------------------------------------------------------------------------
-- UNIT 13 : TRIGGER
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_split_trg
BEFORE INSERT OR UPDATE ON qt_split_data
FOR EACH ROW
BEGIN
    :NEW.note :=
        SUBSTR(
            NVL(:NEW.note, 'TRG-INIT')
            || CASE
                   WHEN INSTR(NVL(:NEW.txt, 'x'), ';') > 0 THEN q'[ | TRG:semicolon ]'
                   ELSE q'{ | TRG:no:semicolon }'
               END,
            1,
            4000
        );

    IF :NEW.amt IS NULL THEN
        :NEW.amt := 0;
    END IF;
END qt_split_trg;
/

--------------------------------------------------------------------------------
-- UNIT 14 : VIEW
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_split_view AS
WITH base AS
(
    SELECT d.id,
           d.grp_code,
           d.txt,
           d.amt,
           d.note,
           ROW_NUMBER() OVER (PARTITION BY d.grp_code ORDER BY d.id) AS rn,
           COUNT(*) OVER (PARTITION BY d.grp_code) AS grp_cnt
    FROM qt_split_data d
),
decorated AS
(
    SELECT b.*,
           CASE
               WHEN REGEXP_LIKE(b.txt, '[/]') THEN 'HAS_SLASH'
               WHEN REGEXP_LIKE(b.txt, '[;]') THEN 'HAS_SEMI'
               ELSE 'PLAIN'
           END AS shape_flag
    FROM base b
)
SELECT id,
       grp_code,
       txt,
       amt,
       grp_cnt,
       rn,
       shape_flag,
       SUBSTR(note, 1, 120) AS note_short
FROM decorated;

/*
    another fake section
    /
    /
    /
    END;
*/

--------------------------------------------------------------------------------
-- UNIT 15 : HEAVY ANONYMOUS RUNNER
--------------------------------------------------------------------------------
DECLARE
    v_msg VARCHAR2(4000);
    v_obj qt_split_obj;
    v_sql VARCHAR2(32767);
BEGIN
    qt_split_pkg.log_msg('ANON.START', 'runner begin');

    qt_split_pkg.seed_more(100);

    qt_split_proc(1.5);

    v_obj := qt_split_pkg.build_obj(
        777,
        'ANON',
        q'{anonymous object ; text / -- /* */ }',
        7.77
    );

    v_msg := v_obj.render || ' | ' || qt_split_fun(q'[ANON;CALL/]');

    v_sql := q'[
        BEGIN
            qt_split_pkg.log_msg(:u, :p);
        END;
    ]';

    EXECUTE IMMEDIATE v_sql USING 'ANON.DYN', v_msg;

    INSERT INTO qt_split_data(id, grp_code, txt, amt, note)
    VALUES
    (
        500,
        'G/500',
        q'[inserted in anonymous block ; / ok]',
        500,
        qt_split_pkg.make_note(500, 'anon insert')
    );

    COMMIT;
END;
/

--------------------------------------------------------------------------------
-- UNIT 16 : SELECT FROM VIEW
--------------------------------------------------------------------------------
SELECT id,
       grp_code,
       shape_flag,
       rn,
       grp_cnt,
       note_short
FROM qt_split_view
ORDER BY grp_code, id;

--------------------------------------------------------------------------------
-- UNIT 17 : LEXICAL TRAP ANONYMOUS BLOCK
--------------------------------------------------------------------------------
DECLARE
    v_q1 VARCHAR2(4000);
    v_q2 VARCHAR2(4000);
BEGIN
    v_q1 := q'[
/ <- this is text, not a terminator
BEGIN
  NULL; -- semicolon inside text
END;
/ <- still text
]';

    v_q2 := 'single-quote payload '' ; / -- /* not comment */';

    <<lvl1>>
    BEGIN
        FOR i IN 1 .. 2 LOOP
            BEGIN
                IF i = 1 THEN
                    NULL;
                ELSE
                    NULL;
                END IF;
            EXCEPTION
                WHEN OTHERS THEN
                    NULL;
            END;
        END LOOP;
    END lvl1;

    qt_split_pkg.log_msg('ANON.LEX', SUBSTR(v_q1 || ' | ' || v_q2, 1, 4000));
END;
/

--------------------------------------------------------------------------------
-- UNIT 18 : UPDATE WITH SUBQUERY + DIVISION
--------------------------------------------------------------------------------
UPDATE qt_split_data d
   SET d.amt = NVL(d.amt, 0)
               + (
                    SELECT COUNT(*) / 2
                    FROM qt_split_data x
                    WHERE x.grp_code = d.grp_code
                 ),
       d.note = SUBSTR(NVL(d.note, '') || q'[ | UPDATE; / ; done ]', 1, 4000)
 WHERE d.id IN
 (
     SELECT id
     FROM qt_split_data
     WHERE MOD(id, 2) = 0
 );

--------------------------------------------------------------------------------
-- UNIT 19 : DELETE WITH ANALYTIC SUBQUERY
--------------------------------------------------------------------------------
DELETE FROM qt_split_data
 WHERE id IN
 (
     SELECT id
     FROM
     (
         SELECT id,
                ROW_NUMBER() OVER (PARTITION BY grp_code ORDER BY id DESC) AS rn
         FROM qt_split_data
         WHERE grp_code LIKE 'G/%'
     )
     WHERE rn > 10
 );

--------------------------------------------------------------------------------
-- UNIT 20 : COMMIT
--------------------------------------------------------------------------------
COMMIT;

--------------------------------------------------------------------------------
-- UNIT 21 : FINAL ROW COUNT
--------------------------------------------------------------------------------
SELECT COUNT(*) AS total_rows
FROM qt_split_data;

--------------------------------------------------------------------------------
-- UNIT 22 : LOG SUMMARY
--------------------------------------------------------------------------------
SELECT unit_name,
       COUNT(*) AS cnt
FROM qt_split_log
GROUP BY unit_name
ORDER BY unit_name;

--------------------------------------------------------------------------------
-- UNIT 23 : LOG DETAIL
--------------------------------------------------------------------------------
SELECT log_id,
       unit_name,
       SUBSTR(payload, 1, 120) AS payload_preview
FROM qt_split_log
ORDER BY log_id;