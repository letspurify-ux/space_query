CREATE OR REPLACE PACKAGE BODY pkg_depth AS
  PROCEDURE sync_data IS
  BEGIN
    UPDATE sample_table
    SET abcd = edfg
    -- comment
    , ghij = klmo
    FROM qwer;
  END;
END pkg_depth;
/
