PROCEDURE A (B IN NUMBER) AS
BEGIN
    SELECT D --4
    FROM E --4
    WHERE F IN (
            --4
            SELECT G -- 12
            FROM ( -- 12
                    SELECT H -- 20
                    FROM J -- 20
                    INNER JOIN K -- 20
                        ON 1 = 1 -- 24
                            AND 2 = 2 -- 28
                            OR 3 = 3 -- 28
                    OUTER JOIN K -- 20
                        ON 1 = 1 -- 24
                            AND 2 = 2 -- 28
                            OR 3 = 3 -- 28
                ) I -- 16
        ); -- 8
END A;

SELECT D
FROM E
WHERE F IN (
        SELECT G --8
        FROM ( --8
                SELECT H --16
                FROM J --16
                INNER JOIN K --16
                    ON 1 = 1 --20
                        AND 2 = 2 -- 24
                OUTER JOIN K -- 16
                    ON 1 = 1 --20
                        AND 2 = 2 -- 24
            ) I --12
    ); --4