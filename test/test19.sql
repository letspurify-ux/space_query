/* 
    FINAL BOSS FOR ORACLE EXECUTION UNIT SPLITTER

    아래는 모두 가짜 종료 패턴이다.
    /
    ;
    END;
    CREATE OR REPLACE PACKAGE nope IS
        PROCEDURE p;
    END;
    /
*/

--------------------------------------------------------------------------------
-- UNIT 01 : CLEANUP
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
    drop_if_exists('drop trigger qt_boss_trg');
    drop_if_exists('drop view qt_boss_view');
    drop_if_exists('drop procedure qt_boss_proc');
    drop_if_exists('drop function qt_boss_fun');
    drop_if_exists('drop package qt_boss_pkg');
    drop_if_exists('drop type qt_boss_obj force');
    drop_if_exists('drop sequence qt_boss_seq');
    drop_if_exists('drop table qt_boss_log purge');
    drop_if_exists('drop table qt_boss_data purge');
END;
   /

--------------------------------------------------------------------------------
-- UNIT 02 : LOG TABLE
--------------------------------------------------------------------------------
CREATE TABLE qt_boss_log
(
    log_id     NUMBER PRIMARY KEY,
    unit_name  VARCHAR2(100) NOT NULL,
    payload    VARCHAR2(4000),
    created_at TIMESTAMP DEFAULT SYSTIMESTAMP NOT NULL
);

--------------------------------------------------------------------------------
-- UNIT 03 : DATA TABLE
--------------------------------------------------------------------------------
CREATE TABLE qt_boss_data
(
    id         NUMBER PRIMARY KEY,
    grp_code   VARCHAR2(30) NOT NULL,
    txt        VARCHAR2(4000),
    amt        NUMBER(18,4),
    note       VARCHAR2(4000),
    created_at DATE DEFAULT SYSDATE NOT NULL
);

--------------------------------------------------------------------------------
-- UNIT 04 : SEQUENCE
--------------------------------------------------------------------------------
CREATE SEQUENCE qt_boss_seq
    START WITH 1
    INCREMENT BY 1
    NOCACHE;

--------------------------------------------------------------------------------
-- UNIT 05 : TYPE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_boss_obj AS OBJECT
(
    id       NUMBER,
    grp_code VARCHAR2(30),
    txt      VARCHAR2(4000),
    MEMBER FUNCTION render RETURN VARCHAR2
);
 /

--------------------------------------------------------------------------------
-- UNIT 06 : TYPE BODY
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE BODY qt_boss_obj AS
    MEMBER FUNCTION render RETURN VARCHAR2 IS
        v_txt VARCHAR2(4000);
    BEGIN
        /*
            fake comment terminators inside TYPE BODY
               /
               ;
               END;
        */
        v_txt :=
            SUBSTR(
                '[' || grp_code || '] ' || txt || q'~ | TYPE-BODY ; / END; ~',
                1,
                4000
            );

        RETURN v_txt;
    END render;
END;
    /

--------------------------------------------------------------------------------
-- UNIT 07 : PACKAGE SPEC
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_boss_pkg AUTHID DEFINER IS
    c_spec_trap CONSTANT VARCHAR2(500) := q'~SPEC-BEGIN
   /
   ;
END;
CREATE OR REPLACE PROCEDURE not_real IS
BEGIN
    NULL;
END;
   /
SPEC-END~';

    PROCEDURE log_msg(p_unit_name VARCHAR2, p_payload VARCHAR2);
    FUNCTION fake_text RETURN VARCHAR2;
    FUNCTION build_obj
    (
        p_id   NUMBER,
        p_grp  VARCHAR2,
        p_txt  VARCHAR2,
        p_amt  NUMBER
    ) RETURN qt_boss_obj;
    PROCEDURE seed_complex(p_base NUMBER);
END qt_boss_pkg;
  /

--------------------------------------------------------------------------------
-- UNIT 08 : PACKAGE BODY
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_boss_pkg IS

    g_body_trap CONSTANT VARCHAR2(32767) := q'~BODY-BEGIN
   /
   ;
END;
CREATE OR REPLACE TRIGGER not_real_trg
BEFORE INSERT ON nowhere
BEGIN
    NULL;
END;
   /
BODY-END~';

    PROCEDURE log_msg(p_unit_name VARCHAR2, p_payload VARCHAR2) IS
        PRAGMA AUTONOMOUS_TRANSACTION;
    BEGIN
        INSERT INTO qt_boss_log
        (
            log_id,
            unit_name,
            payload
        )
        VALUES
        (
            qt_boss_seq.NEXTVAL,
            p_unit_name,
            SUBSTR(p_payload, 1, 4000)
        );

        COMMIT;
    END log_msg;

    FUNCTION fake_text RETURN VARCHAR2 IS
        v_text  VARCHAR2(4000);
        v_regex CONSTANT VARCHAR2(100) := '^\s*/\s*$';
    BEGIN
        v_text :=
            SUBSTR(
                   g_body_trap
                || ' | REGEX=' || v_regex
                || q'~ | ; | / | END; | /* */ | -- ~',
                1,
                4000
            );

        RETURN v_text;
    END fake_text;

    FUNCTION build_obj
    (
        p_id   NUMBER,
        p_grp  VARCHAR2,
        p_txt  VARCHAR2,
        p_amt  NUMBER
    ) RETURN qt_boss_obj
    IS
    BEGIN
        RETURN qt_boss_obj(p_id, p_grp, p_txt);
    END build_obj;

    PROCEDURE seed_complex(p_base NUMBER) IS
        v_sql    VARCHAR2(32767);
        v_plsql  VARCHAR2(32767);
        v_text   VARCHAR2(4000);

        PROCEDURE add_one
        (
            p_offset NUMBER,
            p_grp    VARCHAR2,
            p_txt    VARCHAR2,
            p_amt    NUMBER
        )
        IS
        BEGIN
            v_sql := q'~INSERT INTO qt_boss_data(id, grp_code, txt, amt, note)
                        VALUES (:1, :2, :3, :4, :5)~';

            EXECUTE IMMEDIATE v_sql
                USING p_base + p_offset,
                      p_grp,
                      p_txt,
                      p_amt,
                      SUBSTR(v_text, 1, 4000);
        END add_one;
    BEGIN
        v_text := fake_text;

        add_one(
            1,
            'G/X',
            q'~X-row
   /
   ;
END;
~',
            11
        );

        add_one(
            2,
            'G/Y',
            q'{Y-row
   /
   ;
END;
}',
            22
        );

        add_one(
            3,
            'G/Z',
            q'<Z-row
   /
   ;
END;
>',
            33
        );

        v_plsql := q'~
BEGIN
    qt_boss_pkg.log_msg(:u, :p);
END;
~';

        EXECUTE IMMEDIATE v_plsql
            USING 'PKG.SEED_COMPLEX', SUBSTR(v_text, 1, 4000);

        /*
            fake comment terminators inside PACKAGE BODY
               /
               ;
               END;
            CREATE OR REPLACE FUNCTION fake RETURN NUMBER IS
            BEGIN
                RETURN 1;
            END;
               /
        */
    END seed_complex;

END qt_boss_pkg;
   /

--------------------------------------------------------------------------------
-- UNIT 09 : FUNCTION
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION qt_boss_fun
(
    p_prefix VARCHAR2 DEFAULT q'~FUN-DEFAULT
   /
   ;
END;
~'
)
RETURN VARCHAR2
IS
    v_text VARCHAR2(4000);
BEGIN
    v_text := SUBSTR(p_prefix || ' -> ' || qt_boss_pkg.fake_text, 1, 4000);

    qt_boss_pkg.log_msg('FUNCTION', v_text);

    RETURN v_text;
END qt_boss_fun;
 /

--------------------------------------------------------------------------------
-- UNIT 10 : PROCEDURE
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_boss_proc(p_multiplier NUMBER)
IS
    CURSOR c_data IS
        SELECT id, NVL(amt, 0) AS amt
        FROM qt_boss_data
        WHERE id <= 500
        ORDER BY id;

    v_sum NUMBER := 0;
BEGIN
    <<outer_loop>>
    FOR r IN c_data LOOP
        BEGIN
            v_sum := v_sum + (r.amt * p_multiplier);

            UPDATE qt_boss_data
               SET amt  = NVL(amt, 0)
                          + CASE
                                WHEN MOD(r.id, 2) = 0 THEN p_multiplier
                                ELSE p_multiplier / 2
                            END,
                   note = SUBSTR(
                              NVL(note, '')
                              || q'~ | PROC-TOUCHED ; / END; ~',
                              1,
                              4000
                          )
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
                qt_boss_pkg.log_msg('PROC.ERROR', SQLERRM);
                RAISE;
        END;
    END LOOP outer_loop;

    qt_boss_pkg.log_msg(
        'PROCEDURE',
        'sum=' || TO_CHAR(v_sum) || ' | fn=' || qt_boss_fun
    );
END qt_boss_proc;
   /

--------------------------------------------------------------------------------
-- UNIT 11 : TRIGGER
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_boss_trg
BEFORE INSERT OR UPDATE ON qt_boss_data
FOR EACH ROW
BEGIN
    :NEW.note :=
        SUBSTR(
            NVL(:NEW.note, 'TRG-INIT')
            || CASE
                   WHEN INSTR(NVL(:NEW.txt, 'x'), ';') > 0
                       THEN q'~ | TRG:semicolon ~'
                   ELSE q'~ | TRG:no:semicolon ~'
               END
            || CASE
                   WHEN INSTR(NVL(:NEW.txt, 'x'), '/') > 0
                       THEN q'~ | TRG:slash ~'
                   ELSE q'~ | TRG:no:slash ~'
               END,
            1,
            4000
        );

    IF :NEW.amt IS NULL THEN
        :NEW.amt := 0;
    END IF;
END qt_boss_trg;
  /

--------------------------------------------------------------------------------
-- UNIT 12 : VIEW
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_boss_view AS
WITH base AS
(
    SELECT d.id,
           d.grp_code,
           d.txt,
           d.amt,
           d.note,
           ROW_NUMBER() OVER (PARTITION BY d.grp_code ORDER BY d.id) AS rn,
           COUNT(*)    OVER (PARTITION BY d.grp_code) AS grp_cnt
      FROM qt_boss_data d
      /*
          fake SQL comment terminators
             /
             ;
             END;
      */
),
decorated AS
(
    SELECT b.*,
           CASE
               WHEN INSTR(NVL(b.txt, 'x'), '/') > 0 THEN 'HAS_SLASH'
               WHEN INSTR(NVL(b.txt, 'x'), ';') > 0 THEN 'HAS_SEMI'
               ELSE 'PLAIN'
           END AS shape_flag
      FROM base b
)
SELECT id,
       grp_code,
       amt,
       rn,
       grp_cnt,
       shape_flag,
       SUBSTR(note, 1, 120) AS note_short
  FROM decorated;

--------------------------------------------------------------------------------
-- UNIT 13 : INSERT ALL
--------------------------------------------------------------------------------
INSERT ALL
    INTO qt_boss_data (id, grp_code, txt, amt, note)
    VALUES
    (
        1,
        'G/A',
        q'~alpha
   /
beta ; gamma
END;
~',
        10,
        q'!note-1
   /
   ;
-- not a real comment
/* not a real block comment */
!'
    )
    INTO qt_boss_data (id, grp_code, txt, amt, note)
    VALUES
    (
        2,
        'G/B',
        q'{beta
   /
;
END;
}',
        20,
        q'<note-2
   /
;
END;
>'
    )
    INTO qt_boss_data (id, grp_code, txt, amt, note)
    VALUES
    (
        3,
        'G/C',
        q'[gamma
   /
;
END;
]',
        30,
        'plain note ; / END;'
    )
SELECT 1
FROM dual;

--------------------------------------------------------------------------------
-- UNIT 14 : MERGE
--------------------------------------------------------------------------------
MERGE INTO qt_boss_data d
USING
(
    WITH src AS
    (
        SELECT 2 AS id,
               'G/B' AS grp_code,
               q'~merge-update
   /
;
END;
~' AS txt,
               200 AS amt,
               q'!merge-note
   /
;
END;
!' AS note
          FROM dual
        UNION ALL
        SELECT 4,
               'G/D',
               q'{merge-insert
   /
;
END;
}',
               40,
               q'<new-row
   /
;
END;
>'
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
           d.note = SUBSTR(NVL(d.note, '') || ' | ' || s.note, 1, 4000)
WHEN NOT MATCHED THEN
    INSERT (id, grp_code, txt, amt, note)
    VALUES (s.id, s.grp_code, s.txt, s.amt, s.note);

--------------------------------------------------------------------------------
-- UNIT 15 : HEAVY ANONYMOUS RUNNER
--------------------------------------------------------------------------------
DECLARE
    v_msg VARCHAR2(4000);
    v_obj qt_boss_obj;
    v_sql VARCHAR2(32767);
BEGIN
    qt_boss_pkg.log_msg('ANON.START', 'runner begin');

    qt_boss_pkg.seed_complex(100);

    qt_boss_proc(2);

    v_obj := qt_boss_pkg.build_obj(
        777,
        'ANON',
        q'~anon-object
   /
;
END;
~',
        7.77
    );

    v_msg :=
        SUBSTR(
            v_obj.render || ' | ' || qt_boss_fun(
                q'~ANON-CALL
   /
;
END;
~'
            ),
            1,
            4000
        );

    v_sql := q'~INSERT INTO qt_boss_data(id, grp_code, txt, amt, note)
                VALUES (:1, :2, :3, :4, :5)~';

    EXECUTE IMMEDIATE v_sql
        USING 500,
              'G/500',
              q'~inserted-in-anon
   /
;
END;
~',
              500,
              v_msg;

    COMMIT;
END;
    /

--------------------------------------------------------------------------------
-- UNIT 16 : LEXICAL TRAP ANONYMOUS BLOCK
--------------------------------------------------------------------------------
DECLARE
    v_q1 VARCHAR2(4000);
    v_q2 VARCHAR2(4000);
BEGIN
    v_q1 := q'~this is text
   /
   ;
END;
CREATE OR REPLACE PACKAGE not_real IS
    PROCEDURE p;
END;
   /
~';

    v_q2 := 'single quote payload '' ; / /* not comment */ -- not comment';

    /*
        fake comment terminators
           /
           ;
           END;
    */

    qt_boss_pkg.log_msg('ANON.LEX', SUBSTR(v_q1 || ' | ' || v_q2, 1, 4000));
END;
 /

--------------------------------------------------------------------------------
-- UNIT 17 : SELECT FROM VIEW
--------------------------------------------------------------------------------
SELECT id,
       grp_code,
       rn,
       grp_cnt,
       shape_flag,
       note_short
FROM qt_boss_view
ORDER BY grp_code, id;

--------------------------------------------------------------------------------
-- UNIT 18 : UPDATE
--------------------------------------------------------------------------------
UPDATE qt_boss_data d
   SET d.amt =
           NVL(d.amt, 0)
           + (
                SELECT COUNT(*)
                FROM qt_boss_data x
                WHERE x.grp_code = d.grp_code
             ) / 2,
       d.note =
           SUBSTR(
               NVL(d.note, '') || q'~ | UPDATE ; / END; ~',
               1,
               4000
           )
 WHERE d.id IN
 (
     SELECT id
     FROM qt_boss_data
     WHERE MOD(id, 2) = 0
 );

--------------------------------------------------------------------------------
-- UNIT 19 : DELETE (NO-OP BUT MUST SPLIT CORRECTLY)
--------------------------------------------------------------------------------
DELETE FROM qt_boss_data
 WHERE id IN
 (
     SELECT id
     FROM
     (
         SELECT id,
                ROW_NUMBER() OVER (PARTITION BY grp_code ORDER BY id DESC) AS rn
         FROM qt_boss_data
         WHERE grp_code = 'NO/SUCH/GROUP'
     )
     WHERE rn > 1
 );

--------------------------------------------------------------------------------
-- UNIT 20 : COMMIT
--------------------------------------------------------------------------------
COMMIT;

--------------------------------------------------------------------------------
-- UNIT 21 : VERIFICATION BLOCK
--------------------------------------------------------------------------------
DECLARE
    v_row_cnt      NUMBER;
    v_expected_ids NUMBER;
    v_log_cnt      NUMBER;
    v_func_cnt     NUMBER;
    v_proc_cnt     NUMBER;
    v_seed_cnt     NUMBER;
    v_start_cnt    NUMBER;
    v_lex_cnt      NUMBER;
BEGIN
    SELECT COUNT(*)
      INTO v_row_cnt
      FROM qt_boss_data;

    SELECT COUNT(*)
      INTO v_expected_ids
      FROM qt_boss_data
     WHERE id IN (1, 2, 3, 4, 101, 102, 103, 500);

    SELECT COUNT(*)
      INTO v_log_cnt
      FROM qt_boss_log;

    SELECT COUNT(*)
      INTO v_func_cnt
      FROM qt_boss_log
     WHERE unit_name = 'FUNCTION';

    SELECT COUNT(*)
      INTO v_proc_cnt
      FROM qt_boss_log
     WHERE unit_name = 'PROCEDURE';

    SELECT COUNT(*)
      INTO v_seed_cnt
      FROM qt_boss_log
     WHERE unit_name = 'PKG.SEED_COMPLEX';

    SELECT COUNT(*)
      INTO v_start_cnt
      FROM qt_boss_log
     WHERE unit_name = 'ANON.START';

    SELECT COUNT(*)
      INTO v_lex_cnt
      FROM qt_boss_log
     WHERE unit_name = 'ANON.LEX';

    IF v_row_cnt <> 8 THEN
        raise_application_error(-20001, 'ROW COUNT FAIL: ' || v_row_cnt);
    END IF;

    IF v_expected_ids <> 8 THEN
        raise_application_error(-20002, 'EXPECTED IDS FAIL: ' || v_expected_ids);
    END IF;

    IF v_log_cnt <> 6 THEN
        raise_application_error(-20003, 'LOG COUNT FAIL: ' || v_log_cnt);
    END IF;

    IF v_func_cnt <> 2
       OR v_proc_cnt <> 1
       OR v_seed_cnt <> 1
       OR v_start_cnt <> 1
       OR v_lex_cnt <> 1
    THEN
        raise_application_error(
            -20004,
            'LOG DISTRIBUTION FAIL: '
            || 'F=' || v_func_cnt
            || ', P=' || v_proc_cnt
            || ', S=' || v_seed_cnt
            || ', A=' || v_start_cnt
            || ', L=' || v_lex_cnt
        );
    END IF;
END;
   /

--------------------------------------------------------------------------------
-- UNIT 22 : LOG SUMMARY
--------------------------------------------------------------------------------
SELECT unit_name,
       COUNT(*) AS cnt
FROM qt_boss_log
GROUP BY unit_name
ORDER BY unit_name;

--------------------------------------------------------------------------------
-- UNIT 23 : DATA SUMMARY
--------------------------------------------------------------------------------
SELECT id,
       grp_code,
       amt
FROM qt_boss_data
ORDER BY id;

--------------------------------------------------------------------------------
-- UNIT 24 : PAYLOAD PREVIEW
--------------------------------------------------------------------------------
SELECT log_id,
       unit_name,
       SUBSTR(payload, 1, 120) AS payload_preview
FROM qt_boss_log
ORDER BY log_id;