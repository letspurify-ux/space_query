# 필수 트랜잭션 관리 원칙

대상: `rust_query_tool` 같은 GUI SQL Client  
목적: 사용자가 의도하지 않은 commit/rollback, 미커밋 작업 유실, UI와 실제 DB 세션 상태 불일치를 방지한다.

---

## 1. 트랜잭션은 반드시 물리 DB 세션 기준으로 관리한다

트랜잭션 상태는 탭, 에디터, 쿼리 문자열이 아니라 **실제 DB connection/session**에 묶인다.

따라서 commit, rollback, dirty 표시, 세션 재사용 여부는 모두 실행에 사용된 물리 세션 기준으로 판단해야 한다.

**필수 정책**

- 탭별로 사용한 물리 세션을 명확히 추적한다.
- commit/rollback 대상은 현재 선택 탭이 아니라 "요청 시점에 확정된 탭의 세션"이어야 한다.
- 연결 재생성 후 이전 세션은 stale 처리하고 재사용하지 않는다.

---

## 2. UI 상태와 DB 실제 상태는 항상 일치해야 한다

Auto-commit, transaction mode, dirty 상태는 사용자가 믿고 행동하는 정보다. UI에 표시된 상태와 실제 DB 세션 상태가 다르면 가장 위험하다.

**필수 정책**

- DB에 설정 적용이 성공한 뒤에만 UI/내부 상태를 변경한다.
- `SET autocommit`, `SET TRANSACTION`, session option 변경 실패는 반드시 오류로 전파한다.
- 실패 시 기존 상태를 유지하고, 해당 세션은 필요하면 폐기한다.

---

## 3. Auto-commit 변경은 원자적으로 처리한다

Auto-commit 변경은 단순 옵션 변경이 아니라 트랜잭션 정책 변경이다.

**필수 정책**

- dirty session에서는 auto-commit 변경을 막는다.
- 변경 전 commit 또는 rollback 결정을 요구한다.
- clean session에만 변경을 적용한다.
- retained session이 여러 개라면 모두 적용하거나, 적용 불가한 session은 폐기한다.
- 적용 실패 시 일부 세션만 다른 auto-commit 상태로 남겨두지 않는다.

---

## 4. Dirty session은 사용자 결정 없이 정리하지 않는다

미커밋 가능성이 있는 세션은 자동으로 조용히 commit하거나 rollback하면 안 된다.

**필수 정책**

- DML, procedure, script, manual transaction control 실행 후에는 dirty 가능성을 보수적으로 표시한다.
- 탭 닫기, 앱 종료, 연결 해제 전 dirty session이 있으면 commit/rollback/discard 선택을 요구한다.
- 사용자가 결정을 내리기 전에는 해당 세션을 안전한 clean session처럼 재사용하지 않는다.

---

## 5. 확실하지 않으면 안전하지 않은 것으로 간주한다

DB 드라이버가 트랜잭션 상태를 명확히 알려주지 않는 경우가 많다. 이때 낙관적으로 clean 처리하면 안 된다.

**필수 정책**

- 트랜잭션 상태를 확정할 수 없으면 `MaybeDirty` 또는 `DecisionRequired`로 둔다.
- health check 성공은 "연결이 살아 있음"만 의미한다.
- health check 성공을 "트랜잭션이 clean함"으로 해석하지 않는다.

---

## 6. Cancel/timeout 이후 세션 재사용은 보수적으로 판단한다

취소나 타임아웃 이후에는 쿼리 실행, cursor, lock, transaction 상태가 애매해질 수 있다.

**필수 정책**

- DML/procedure/script 실행 중 cancel/timeout이 발생하면 commit/rollback 결정을 요구하거나 세션을 폐기한다.
- SELECT 계열도 cursor 정리, timeout 복원, health check가 모두 성공한 경우에만 재사용한다.
- worker가 아직 끝나지 않았거나 상태 확인이 불가능하면 세션을 재사용하지 않는다.

---

## 7. Transaction mode 변경은 clean 상태에서만 허용한다

Isolation level, read-only/read-write 같은 transaction mode는 DB마다 적용 시점과 제약이 다르다.

**필수 정책**

- dirty session 또는 decision-required session에서는 transaction mode 변경을 막는다.
- Oracle처럼 first statement 제약이 있는 설정은 새 물리 세션에서 적용하는 것을 원칙으로 한다.
- MySQL/MariaDB처럼 "다음 트랜잭션부터 적용"되는 설정은 UI에 현재 트랜잭션 적용 여부를 명확히 구분한다.

---

## 8. 수동 트랜잭션 제어 문장은 별도 처리한다

사용자가 직접 `COMMIT`, `ROLLBACK`, `BEGIN`, `START TRANSACTION`, `SAVEPOINT`, `SET autocomMIT` 등을 실행할 수 있다. 이 문장들은 일반 DML과 다르게 상태 전이를 만든다.

**필수 정책**

| 문장 유형 | 성공 후 필수 상태 처리 |
|---|---|
| `COMMIT` | clean |
| `ROLLBACK` | clean |
| `BEGIN`, `START TRANSACTION` | dirty 또는 transaction-open |
| `SAVEPOINT` | dirty 유지 |
| `ROLLBACK TO SAVEPOINT` | dirty 유지 |
| `SET autocommit` | 실제 DB 적용 성공 여부 확인 후 상태 반영 |
| 알 수 없는 transaction control | decision-required |

---

## 9. DDL과 implicit commit을 DB별로 분리해 처리한다

DDL은 DB마다 transaction에 미치는 영향이 다르다. 어떤 DB에서는 DDL이 암묵적으로 commit을 발생시킨다.

**필수 정책**

- DDL 실행 후 상태 전이는 DB별 정책으로 판단한다.
- implicit commit 가능성이 있으면 dirty 상태를 단순 유지하지 말고 실제 의미를 명확히 정의한다.
- DDL 실패 시에도 이전 미커밋 작업이 남아 있을 수 있으므로 보수적으로 처리한다.
- DDL/SessionControl interrupt에서 이전 dirty/session-bound 상태가 없으면 rollback 결정을 제시하지 않고 물리 세션을 교체한다. DDL은 implicit commit 또는 부분 적용 가능성이 있어 rollback으로 복구된다고 오해시키면 안 된다.

---

## 10. 상태 전이는 중앙 정책으로 관리한다

트랜잭션 상태 판단이 UI, 실행기, cleanup, pool 저장 로직에 흩어지면 정책이 어긋나기 쉽다.

**필수 정책**

- `Clean`, `MaybeDirty`, `DecisionRequired` 같은 중앙 상태 모델을 둔다.
- 모든 실행 결과는 중앙 상태 전이 함수를 통해 반영한다.
- UI는 상태를 직접 추론하지 말고 중앙 상태만 표시한다.
- 세션 저장/폐기/교체도 중앙 정책 결과에 따라 결정한다.

---

## 최소 상태 모델

```text
Clean
  └─ 미커밋 작업 없음

MaybeDirty
  └─ 미커밋 가능성 있음

DecisionRequired
  └─ commit/rollback/discard 결정 필요

InvalidSession
  └─ 재사용 금지, 물리 세션 폐기 필요
```

---

## 최소 구현 체크리스트

- [ ] Auto-commit 적용 실패를 무시하지 않고 `Result`로 전파한다.
- [ ] DB 적용 성공 후에만 UI 상태를 변경한다.
- [ ] Dirty session에서는 auto-commit/transaction mode 변경을 막는다.
- [ ] Commit/rollback 대상 세션을 요청 시점에 고정한다.
- [ ] Cancel/timeout 이후에는 확실히 안전한 경우에만 세션을 재사용한다.
- [ ] Health check와 transaction clean 여부를 분리한다.
- [ ] 수동 `COMMIT`/`ROLLBACK`/`BEGIN`/`SAVEPOINT` 문장을 별도 상태 전이로 처리한다.
- [ ] DDL implicit commit 정책을 DB별로 정의한다.
- [ ] Dirty 또는 decision-required session이 있는 탭은 닫기 전 사용자 결정을 요구한다.
- [ ] 상태 판단 로직을 중앙 상태 머신으로 통합한다.

---

## 한 줄 원칙

**트랜잭션 상태를 확신할 수 없으면 clean으로 간주하지 말고, 사용자에게 commit/rollback 결정을 요구하거나 물리 세션을 폐기한다.**
