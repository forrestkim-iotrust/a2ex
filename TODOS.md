# TODOS

## P0 — Must have for MVP

### Backup gate on terminate
- **What:** Terminate 플로우에서 encrypted_backup 존재 여부 확인. 없으면 에이전트에 백업 명령 후 대기.
- **Why:** 컨테이너 종료 시 WAIaaS 키샤드 소멸 → 자금 영구 손실. 2026-03-24에 실제 5 USDC 손실 이력.
- **Context:** terminate/route.ts에서 closeAkashDeployment() 호출 전에 deployments.encrypted_backup 컬럼 체크 추가. backup 없으면 callback으로 "backup_now" 명령 → 30초 대기 → 타임아웃 시 사용자에게 경고 후 결정 위임.
- **Depends on:** 서명 기반 백업 키 구현 (아래)

### Signature-based WAIaaS backup
- **What:** SIWE 후 personal_sign으로 백업 키 생성 → AES-256-GCM으로 WAIaaS 데이터 암호화 → Neon DB 저장.
- **Why:** 컨테이너 종료/크래시 시 자금 복구 경로 확보.
- **Context:** 비급행. Plugin heartbeat에서 주기적으로 시도, 실패해도 재시도. 성공할 때까지 반복. terminate 시 게이트만 걸면 됨. 변경 파일: schema.ts(encrypted_backup 컬럼), siwe/route.ts, session.ts, sdl.ts, callback/route.ts(backup type), plugin/index.ts.
- **Depends on:** 없음

### SDL 시크릿 제거 → callback 기반 전달
- **What:** OPENROUTER_API_KEY, WAIAAS_MASTER_PASSWORD를 SDL에서 제거. DB에 저장하고 플러그인이 callback GET으로 fetch.
- **Why:** SDL은 Akash 온체인에 기록됨. 악의적 provider가 키를 빼갈 수 있음. Callback 채널은 CALLBACK_TOKEN으로 인증됨.
- **Context:** SDL에 남는 건 CALLBACK_TOKEN, CALLBACK_URL, DEPLOYMENT_ID, STRATEGY_ID, FUND_LIMIT_USD, RISK_LEVEL만. 플러그인 부팅 시 callback GET에 `type: "secrets"` 요청 → 서버가 DB에서 조회 후 응답. callback/route.ts에 새 GET 타입 추가 + sdl.ts에서 env var 제거.
- **Depends on:** 없음

### USDC 입금 확인 UX
- **What:** 에이전트가 MPC 지갑 USDC 잔액을 callback으로 보고 → 대시보드에 표시.
- **Why:** 유저가 USDC를 보냈는데 피드백이 없으면 불안. 신뢰감 필수.
- **Context:** callback type: "balance_update" 추가. 대시보드 사이드바에 "Hot Wallet: $5.00 USDC" 표시.
- **Depends on:** 없음

### 구조화된 로깅
- **What:** deploy, terminate, callback 라우트에 JSON 구조화 로그 추가.
- **Why:** 버그 리포트 시 로그만으로 재구성 가능해야 함. 현재 console.error 1줄만 존재.
- **Context:** Vercel 로그에서 검색 가능한 형식. { action, deploymentId, userAddress, result, error }.
- **Depends on:** 없음

### 대시보드 백업 상태 표시
- **What:** heartbeat에 마지막 백업 시간 포함 → 대시보드에 "Last backup: 2min ago" 표시.
- **Why:** 백업 실패 시 사용자가 인지하고 수동 대응 가능.
- **Context:** heartbeat payload에 lastBackupAt 필드 추가. 대시보드 사이드바에 표시.
- **Depends on:** 백업 시스템 구현

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
