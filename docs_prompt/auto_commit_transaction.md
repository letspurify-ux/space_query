트랜잭션 및 read/write 설정 기능, auto commit 기능에서 버그가 있을지, 추가로 필요한 테스트가 있을지 검토해줘.

테스트할 때는 도커 이미지를 하나씩만 띄워줘. 메모리가 부족해.

모든 테스트 케이스가 oracle oci/thin, mariadb, mysql에서 모두 정상 동작하는 것이 보장되어야 해. 결과는 테이블 형태로 정리해서 최종 보고해줘.
row는 테스트 케이스, col은 oracle oci, thin, mariadb, mysql로 구분해줘.
