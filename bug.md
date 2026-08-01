# 발견된 프로그램 버그

## Oracle Thin/OCI 비교 하네스가 중첩 커서의 시각 값을 정규화하지 않음

- 재현: `target/debug/oracle_compare_test_all test/test20.sql` 또는 `target/debug/oracle_compare_test_all test/test21.sql`
- 현상: Thin과 OCI 모두 12개 그리드를 정상 반환하지만, 중첩 `SYS_REFCURSOR`가 JSON으로 직렬화된 셀 안의 `CREATED_AT` 값이 실행 시각만큼 달라서 `Oracle Thin select cells differ from OCI`로 실패한다.
- 기대 동작: 최상위 날짜/타임스탬프 셀처럼 허용 오차를 적용하거나, 중첩 커서 JSON을 구조적으로 비교하면서 날짜/타임스탬프 값을 정규화해야 한다.
- 현재 쿼리 우회: `test/test20.sql`과 `test/test21.sql`의 중첩 커서 결과에서 비결정적인 `created_at` 열을 제외한다.
