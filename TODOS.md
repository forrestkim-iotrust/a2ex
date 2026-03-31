# TODOS

## P0 — Must have for MVP

All P0 items implemented in commit 0e50d4d (2026-03-31).

- ~~Backup gate on terminate~~ ✅
- ~~Signature-based WAIaaS backup~~ ✅
- ~~SDL 시크릿 제거 → callback 기반 전달~~ ✅
- ~~USDC 입금 확인 UX~~ ✅
- ~~구조화된 로깅~~ ✅
- ~~대시보드 백업 상태 표시~~ ✅

## P1 — Important

### Akash provider failover
- **What:** Heartbeat timeout 감지 → 백업에서 WAIaaS 복원 → 새 provider에 자동 재배포.
- **Why:** Provider가 죽으면 에이전트가 오픈 포지션 상태로 사라짐.
- **Context:** OpenClaw + WAIaaS 상태 복원의 복잡도가 높음. 두 서비스의 상태 구조를 먼저 파악해야 함. 복잡하면 수동 복구로 전환.
- **Depends on:** 백업 구현, recovery flow

## P2 — Nice to have

### Stats cron job
- **What:** Vercel Cron 5분 간격. deployments + trades 집계 → stats_snapshots INSERT.
- **Why:** 랜딩 페이지 실시간 통계 (총 에이전트, AUM, 거래량, PnL).
- **Context:** 스키마와 GET /api/stats 이미 있음. 유저 5명+ 임계값 이하면 숨김.
- **Depends on:** 없음

### 과금 모델 + 유저 비용 관리
- **What:** 컨테이너 운영 비용을 유저에게 청구하는 모델. 미납 시 며칠 유예 후 종료.
- **Why:** 스케일 시 Akash 비용이 유저 입금액을 초과할 수 있음.
- **Context:** MVP에서는 초대제로 비용 흡수. 유료화 시점에 설계.
- **Depends on:** 없음
