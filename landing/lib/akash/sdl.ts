import yaml from "js-yaml";

export interface DeployConfig {
  strategyId: string;
  fundAmountUsd: number;
  riskLevel: string;
  openclawGatewayToken: string;
  callbackUrl: string;
  deploymentId: string;
  callbackToken: string;
}

export function buildSDL(config: DeployConfig): string {
  const sdl = {
    version: "2.0",
    services: {
      "a2ex-agent": {
        image: "ghcr.io/forrestkim-iotrust/a2ex:sha-b345cc6",
        env: [
          `STRATEGY_ID=${sanitize(config.strategyId)}`,
          `FUND_LIMIT_USD=${Math.min(Math.max(config.fundAmountUsd, 10), 1000)}`,
          `RISK_LEVEL=${sanitize(config.riskLevel)}`,
          `OPENCLAW_GATEWAY_TOKEN=${config.openclawGatewayToken}`,
          `CALLBACK_URL=${config.callbackUrl}`,
          `DEPLOYMENT_ID=${config.deploymentId}`,
          `CALLBACK_TOKEN=${config.callbackToken}`,
        ],
        expose: [
          { port: 18789, as: 18789, to: [{ global: true }] },
        ],
      },
    },
    profiles: {
      compute: {
        "a2ex-agent": {
          resources: {
            cpu: { units: "1" },
            memory: { size: "4Gi" },
            storage: [{ size: "10Gi" }],
          },
        },
      },
      placement: {
        dcloud: {
          pricing: {
            "a2ex-agent": { denom: "uakt", amount: 1000 },
          },
        },
      },
    },
    deployment: {
      "a2ex-agent": {
        dcloud: { profile: "a2ex-agent", count: 1 },
      },
    },
  };

  return yaml.dump(sdl);
}

function sanitize(input: string): string {
  return input.replace(/[^a-zA-Z0-9_-]/g, "");
}
