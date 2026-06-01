--------------------------------------------------------------------------------
-- EXECUTION UNIT SPLITTER : FINAL BOSS
-- 예상 실행단위 수: 18
-- 의도:
--   1) 문자열/주석/q-quote 안의 ; 와 / 를 절대 분리 기준으로 오인하면 안 됨
--   2) CREATE OR REPLACE {PACKAGE|PACKAGE BODY|TYPE|PROCEDURE|FUNCTION|TRIGGER}
--      는 내부 세미콜론이 많아도 전체를 1개 단위로 보고, 마지막 "/" 에서 종료해야 함
--   3) 일반 SQL(SELECT/INSERT/UPDATE/MERGE/CREATE VIEW/CREATE TABLE 등)은 ";" 에서 종료
--   4) 익명 PL/SQL 블록은 "END;" 뒤의 "/" 에서 종료
--------------------------------------------------------------------------------

--------------------------------------------------------------------------------
-- UNIT 01 : 일반 SQL 종료는 세미콜론
--------------------------------------------------------------------------------
ALTER SESSION SET NLS_DATE_FORMAT = 'YYYY-MM-DD HH24:MI:SS';

--------------------------------------------------------------------------------
-- UNIT 02 : 익명 블록 + 동적 DROP + 문자열 안의 ; / BEGIN END
--------------------------------------------------------------------------------
DECLARE
    PROCEDURE safe_exec(p_sql IN VARCHAR2) IS
    BEGIN
        EXECUTE IMMEDIATE p_sql;
    EXCEPTION
        WHEN OTHERS THEN
            NULL;
    END;
BEGIN
    safe_exec('DROP TRIGGER qt_split_trg');
    safe_exec('DROP VIEW qt_split_v');
    safe_exec('DROP PACKAGE qt_split_pkg');
    safe_exec('DROP PROCEDURE qt_split_proc');
    safe_exec('DROP FUNCTION qt_split_fn');
    safe_exec('DROP TYPE qt_split_tab');
    safe_exec('DROP TYPE qt_split_obj');
    safe_exec('DROP TABLE qt_split_logs PURGE');
    safe_exec('DROP TABLE qt_split_users PURGE');
    safe_exec('DROP SEQUENCE qt_split_seq');

    -- 아래 문자열은 분리기에 함정용
    safe_exec('BEGIN NULL; END;');
    safe_exec(q'[DECLARE
                     v_txt VARCHAR2(200) := 'not a delimiter ; / BEGIN END';
                 BEGIN
                     NULL;
                 END;]');
END;
/

--------------------------------------------------------------------------------
-- UNIT 03 : CREATE TABLE
--------------------------------------------------------------------------------
CREATE TABLE qt_split_users
(
    user_id        NUMBER        NOT NULL,
    user_name      VARCHAR2(100) NOT NULL,
    status_cd      VARCHAR2(1)   NOT NULL,
    note           CLOB,
    created_at     DATE          DEFAULT SYSDATE NOT NULL,
    updated_at     DATE,
    CONSTRAINT qt_split_users_pk PRIMARY KEY (user_id),
    CONSTRAINT qt_split_users_ck CHECK (status_cd IN ('A', 'I', 'X'))
);

--------------------------------------------------------------------------------
-- UNIT 04 : CREATE TABLE
--------------------------------------------------------------------------------
CREATE TABLE qt_split_logs
(
    log_id         NUMBER        NOT NULL,
    module_name    VARCHAR2(100) NOT NULL,
    msg_text       CLOB,
    created_at     DATE          DEFAULT SYSDATE NOT NULL,
    CONSTRAINT qt_split_logs_pk PRIMARY KEY (log_id)
);

--------------------------------------------------------------------------------
-- UNIT 05 : CREATE SEQUENCE
--------------------------------------------------------------------------------
CREATE SEQUENCE qt_split_seq
START WITH 1
INCREMENT BY 1
NOCACHE;

--------------------------------------------------------------------------------
-- UNIT 06 : INSERT ALL + 문자열/q-quote 내부의 ; / 주석문자 패턴
--------------------------------------------------------------------------------
INSERT ALL
    INTO qt_split_users (user_id, user_name, status_cd, note, updated_at)
    VALUES
    (
        1,
        'ALPHA',
        'A',
        q'[
note line 1 ; semicolon should not split
note line 2 / slash should not split
note line 3 says: BEGIN NULL; END;
note line 4 says: /* not a real comment inside string */
note line 5 says: -- not a line comment inside string
]',
        SYSDATE
    )
    INTO qt_split_users (user_id, user_name, status_cd, note, updated_at)
    VALUES
    (
        2,
        'BETA',
        'I',
        q'!json-ish {"k1":"v;1","k2":"v/2","k3":"BEGIN END;"}!',
        SYSDATE
    )
    INTO qt_split_logs (log_id, module_name, msg_text)
    VALUES
    (
        qt_split_seq.NEXTVAL,
        'BOOT',
        q'[
insert-all boot message;
this slash / is not a unit delimiter;
END; is still text.
]'
    )
SELECT 1
FROM dual;

--------------------------------------------------------------------------------
-- UNIT 07 : TYPE OBJECT -> 마지막 "/" 까지 1개 단위
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_split_obj AS OBJECT
(
    id    NUMBER,
    txt   VARCHAR2(4000)
);
/

--------------------------------------------------------------------------------
-- UNIT 08 : TYPE TABLE -> 마지막 "/" 까지 1개 단위
--------------------------------------------------------------------------------
CREATE OR REPLACE TYPE qt_split_tab AS TABLE OF qt_split_obj;
/

--------------------------------------------------------------------------------
-- UNIT 09 : PACKAGE SPEC -> 내부 세미콜론 다 무시, 마지막 "/" 에서 종료
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE qt_split_pkg
IS
    SUBTYPE t_status IS VARCHAR2(1);

    g_banner CONSTANT VARCHAR2(4000) :=
        q'[
PKG BANNER ;
/
BEGIN
END;
CREATE OR REPLACE PACKAGE BODY fake IS
BEGIN
    NULL;
END;
]';

    PROCEDURE seed_logs(p_times IN PLS_INTEGER DEFAULT 2);

    FUNCTION get_note(p_user_id IN NUMBER)
        RETURN CLOB;

    PROCEDURE complex_upsert
    (
        p_user_id IN NUMBER,
        p_name    IN VARCHAR2,
        p_status  IN t_status,
        p_note    IN CLOB
    );
END qt_split_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 10 : PACKAGE BODY -> 최악 난이도
--------------------------------------------------------------------------------
CREATE OR REPLACE PACKAGE BODY qt_split_pkg
IS
    FUNCTION weird_text
        RETURN CLOB
    IS
        v_txt CLOB;
    BEGIN
        v_txt := q'[
line-1 ; inside q quote
line-2 / inside q quote
line-3 BEGIN
line-4 END;
line-5 /* still text */
line-6 -- still text
/]';
        RETURN v_txt;
    END weird_text;

    PROCEDURE write_log
    (
        p_module IN VARCHAR2,
        p_msg    IN CLOB
    )
    IS
    BEGIN
        INSERT INTO qt_split_logs
        (
            log_id,
            module_name,
            msg_text,
            created_at
        )
        VALUES
        (
            qt_split_seq.NEXTVAL,
            p_module,
            p_msg,
            SYSDATE
        );
    END write_log;

    PROCEDURE seed_logs(p_times IN PLS_INTEGER DEFAULT 2)
    IS
        v_sql   CLOB;
        v_msg   CLOB;
    BEGIN
        FOR i IN 1 .. p_times LOOP
            v_msg := 'seed_logs iteration=' || i || '; banner=' || SUBSTR(g_banner, 1, 60);

            write_log('SEED', v_msg);

            v_sql := q'[
DECLARE
    v_inner_msg VARCHAR2(4000) := 'inner dynamic ; block / not delimiter';
BEGIN
    INSERT INTO qt_split_logs(log_id, module_name, msg_text, created_at)
    VALUES (qt_split_seq.NEXTVAL, 'DYN_INNER', v_inner_msg, SYSDATE);
END;]';

            EXECUTE IMMEDIATE v_sql;
        END LOOP;
    END seed_logs;

    FUNCTION get_note(p_user_id IN NUMBER)
        RETURN CLOB
    IS
        v_note CLOB;
    BEGIN
        SELECT u.note
          INTO v_note
          FROM qt_split_users u
         WHERE u.user_id = p_user_id;

        RETURN v_note;
    EXCEPTION
        WHEN NO_DATA_FOUND THEN
            RETURN q'[NO_DATA_FOUND ; / BEGIN END]';
    END get_note;

    PROCEDURE complex_upsert
    (
        p_user_id IN NUMBER,
        p_name    IN VARCHAR2,
        p_status  IN t_status,
        p_note    IN CLOB
    )
    IS
        v_dummy NUMBER := 0;
    BEGIN
        <<outer_block>>
        DECLARE
            v_local_note CLOB := p_note || CHR(10) || weird_text();
        BEGIN
            MERGE INTO qt_split_users t
            USING
            (
                SELECT p_user_id AS user_id,
                       p_name    AS user_name,
                       p_status  AS status_cd,
                       v_local_note AS note
                  FROM dual
            ) s
               ON (t.user_id = s.user_id)
            WHEN MATCHED THEN
                UPDATE
                   SET t.user_name  = s.user_name,
                       t.status_cd  = s.status_cd,
                       t.note       = s.note,
                       t.updated_at = SYSDATE
            WHEN NOT MATCHED THEN
                INSERT
                (
                    user_id,
                    user_name,
                    status_cd,
                    note,
                    created_at,
                    updated_at
                )
                VALUES
                (
                    s.user_id,
                    s.user_name,
                    s.status_cd,
                    s.note,
                    SYSDATE,
                    SYSDATE
                );

            CASE
                WHEN p_status = 'A' THEN
                    v_dummy := 1;
                WHEN p_status = 'I' THEN
                    v_dummy := 2;
                ELSE
                    v_dummy := 3;
            END CASE;

            write_log
            (
                'UPSERT',
                'user=' || p_user_id || '; status=' || p_status || '; dummy=' || v_dummy
            );
        EXCEPTION
            WHEN OTHERS THEN
                write_log('UPSERT_ERR', 'ERR=' || SQLERRM || '; user=' || p_user_id);
                RAISE;
        END outer_block;
    END complex_upsert;
END qt_split_pkg;
/

--------------------------------------------------------------------------------
-- UNIT 11 : TRIGGER -> "/" 전까지 1단위
--------------------------------------------------------------------------------
CREATE OR REPLACE TRIGGER qt_split_trg
BEFORE INSERT OR UPDATE
ON qt_split_users
FOR EACH ROW
DECLARE
    v_audit_msg CLOB;
BEGIN
    IF INSERTING THEN
        IF :NEW.user_id IS NULL THEN
            :NEW.user_id := qt_split_seq.NEXTVAL;
        END IF;

        IF :NEW.created_at IS NULL THEN
            :NEW.created_at := SYSDATE;
        END IF;
    END IF;

    :NEW.updated_at := SYSDATE;

    v_audit_msg :=
        q'[
trigger fired ;
/
BEGIN END;
]' || ' user_id=' || :NEW.user_id || ', status=' || :NEW.status_cd;

    INSERT INTO qt_split_logs
    (
        log_id,
        module_name,
        msg_text,
        created_at
    )
    VALUES
    (
        qt_split_seq.NEXTVAL,
        'TRIGGER',
        v_audit_msg,
        SYSDATE
    );
END qt_split_trg;
/

--------------------------------------------------------------------------------
-- UNIT 12 : CREATE VIEW -> 세미콜론 종료
--------------------------------------------------------------------------------
CREATE OR REPLACE VIEW qt_split_v
AS
WITH base AS
(
    SELECT
        u.user_id,
        u.user_name,
        u.status_cd,
        u.note,
        u.created_at,
        u.updated_at,
        CASE
            WHEN INSTR(u.note, ';') > 0 THEN 'Y'
            ELSE 'N'
        END AS has_semicolon,
        CASE
            WHEN INSTR(u.note, '/') > 0 THEN 'Y'
            ELSE 'N'
        END AS has_slash
    FROM qt_split_users u
),
agg AS
(
    SELECT
        b.status_cd,
        COUNT(*) AS cnt
    FROM base b
    GROUP BY b.status_cd
)
SELECT
    b.user_id,
    b.user_name,
    b.status_cd,
    b.has_semicolon,
    b.has_slash,
    (
        SELECT a.cnt
        FROM agg a
        WHERE a.status_cd = b.status_cd
    ) AS same_status_cnt,
    SUBSTR(b.note, 1, 120) AS note_preview
FROM base b;

--------------------------------------------------------------------------------
-- UNIT 13 : MERGE -> 세미콜론 종료
--------------------------------------------------------------------------------
MERGE INTO qt_split_users t
USING
(
    SELECT 2 AS user_id,
           'BETA_MERGED' AS user_name,
           'A' AS status_cd,
           q'[merge note ; / BEGIN END]' AS note
      FROM dual
    UNION ALL
    SELECT 3 AS user_id,
           'GAMMA' AS user_name,
           'X' AS status_cd,
           q'[new row from merge ; / text]' AS note
      FROM dual
) s
   ON (t.user_id = s.user_id)
WHEN MATCHED THEN
    UPDATE
       SET t.user_name  = s.user_name,
           t.status_cd  = s.status_cd,
           t.note       = s.note,
           t.updated_at = SYSDATE
WHEN NOT MATCHED THEN
    INSERT
    (
        user_id,
        user_name,
        status_cd,
        note,
        created_at,
        updated_at
    )
    VALUES
    (
        s.user_id,
        s.user_name,
        s.status_cd,
        s.note,
        SYSDATE,
        SYSDATE
    );

--------------------------------------------------------------------------------
-- UNIT 14 : 익명 블록(동적 SQL 안에 또 익명 블록)
--------------------------------------------------------------------------------
DECLARE
    TYPE t_num_tab IS TABLE OF NUMBER INDEX BY PLS_INTEGER;
    v_ids        t_num_tab;
    v_block      CLOB;
    v_count      NUMBER := 0;

    PROCEDURE log_local(p_msg IN CLOB) IS
    BEGIN
        INSERT INTO qt_split_logs(log_id, module_name, msg_text, created_at)
        VALUES (qt_split_seq.NEXTVAL, 'ANON_A', p_msg, SYSDATE);
    END;

    FUNCTION make_block(p_idx NUMBER) RETURN CLOB IS
    BEGIN
        RETURN q'[
DECLARE
    v_txt VARCHAR2(4000) := 'inner anonymous ; block / text';
BEGIN
    INSERT INTO qt_split_logs(log_id, module_name, msg_text, created_at)
    VALUES (qt_split_seq.NEXTVAL, 'INNER_BLOCK', v_txt || ' idx=]' || p_idx || q'[', SYSDATE);
END;]';
    END;
BEGIN
    v_ids(1) := 10;
    v_ids(2) := 11;
    v_ids(3) := 12;

    <<outer_loop>>
    FOR i IN 1 .. v_ids.COUNT LOOP
        BEGIN
            qt_split_pkg.complex_upsert
            (
                p_user_id => v_ids(i),
                p_name    => 'USER_' || v_ids(i),
                p_status  => CASE WHEN MOD(v_ids(i), 2) = 0 THEN 'A' ELSE 'I' END,
                p_note    => 'generated in anon block; idx=' || i || ' / id=' || v_ids(i)
            );

            v_block := make_block(i);
            EXECUTE IMMEDIATE v_block;
            v_count := v_count + 1;
        EXCEPTION
            WHEN OTHERS THEN
                log_local('loop error; i=' || i || '; err=' || SQLERRM);
        END;
    END LOOP outer_loop;

    log_local('anon done; count=' || v_count || '; qt_split_pkg.get_note(1)=' || DBMS_LOB.SUBSTR(qt_split_pkg.get_note(1), 80, 1));
END;
/

--------------------------------------------------------------------------------
-- UNIT 15 : PROCEDURE -> "/" 종료
--------------------------------------------------------------------------------
CREATE OR REPLACE PROCEDURE qt_split_proc
(
    p_status IN VARCHAR2,
    p_rc     OUT SYS_REFCURSOR
)
IS
BEGIN
    OPEN p_rc FOR
        SELECT
            v.user_id,
            v.user_name,
            v.status_cd,
            CASE
                WHEN v.note_preview LIKE '%;%' THEN 'HAS_SEMI'
                WHEN v.note_preview LIKE '%/%' THEN 'HAS_SLASH'
                ELSE 'PLAIN'
            END AS note_kind
        FROM qt_split_v v
        WHERE v.status_cd = p_status
        ORDER BY v.user_id;
END qt_split_proc;
/

--------------------------------------------------------------------------------
-- UNIT 16 : FUNCTION -> "/" 종료
--------------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION qt_split_fn
(
    p_user_id IN NUMBER
)
RETURN CLOB
IS
    v_result CLOB;
BEGIN
    SELECT
        'USER_ID=' || u.user_id
        || ';NAME=' || u.user_name
        || ';STATUS=' || u.status_cd
        || ';NOTE=' || DBMS_LOB.SUBSTR(u.note, 200, 1)
      INTO v_result
      FROM qt_split_users u
     WHERE u.user_id = p_user_id;

    RETURN v_result || q'[ ; function-tail / BEGIN END ]';
EXCEPTION
    WHEN NO_DATA_FOUND THEN
        RETURN q'[NOT_FOUND ; / fn]';
END qt_split_fn;
/

--------------------------------------------------------------------------------
-- UNIT 17 : 익명 블록(프로시저 호출/함수 호출/REF CURSOR/예외/내부블록)
--------------------------------------------------------------------------------
DECLARE
    v_rc          SYS_REFCURSOR;
    v_user_id     NUMBER;
    v_user_name   VARCHAR2(100);
    v_status_cd   VARCHAR2(1);
    v_note_kind   VARCHAR2(30);
    v_fn_txt      CLOB;
BEGIN
    qt_split_pkg.seed_logs(3);

    qt_split_proc('A', v_rc);

    LOOP
        FETCH v_rc
            INTO v_user_id, v_user_name, v_status_cd, v_note_kind;
        EXIT WHEN v_rc%NOTFOUND;

        v_fn_txt := qt_split_fn(v_user_id);

        INSERT INTO qt_split_logs
        (
            log_id,
            module_name,
            msg_text,
            created_at
        )
        VALUES
        (
            qt_split_seq.NEXTVAL,
            'ANON_B',
            'row=' || v_user_id || '; name=' || v_user_name || '; kind=' || v_note_kind
            || '; fn=' || DBMS_LOB.SUBSTR(v_fn_txt, 180, 1),
            SYSDATE
        );
    END LOOP;

    CLOSE v_rc;

    BEGIN
        INSERT INTO qt_split_users(user_id, user_name, status_cd, note)
        VALUES (1, 'DUPLICATE_TEST', 'A', 'this should fail; but block must remain one unit');
    EXCEPTION
        WHEN DUP_VAL_ON_INDEX THEN
            INSERT INTO qt_split_logs(log_id, module_name, msg_text, created_at)
            VALUES
            (
                qt_split_seq.NEXTVAL,
                'ANON_B',
                'expected dup_val_on_index handled; splitter must ignore this inner semicolon structure',
                SYSDATE
            );
    END;
END;
/

--------------------------------------------------------------------------------
-- UNIT 18 : 일반 SELECT -> 세미콜론 종료
--------------------------------------------------------------------------------
SELECT
    l.log_id,
    l.module_name,
    SUBSTR(l.msg_text, 1, 120) AS msg_preview,
    l.created_at
FROM qt_split_logs l
ORDER BY l.log_id;

PROMPT END