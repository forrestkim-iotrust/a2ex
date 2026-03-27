export interface DeployConfig {
  strategyId: string;
  fundAmountUsd: number;
  riskLevel: string;
}

export function buildSDL(config: DeployConfig): string {
  // Structured SDL builder — no string interpolation from user input
  const sdl = {
    version: "2.0",
    services: {
      "a2ex-agent": {
        image: "ghcr.io/iotrust/a2ex:latest",
        env: [
          `STRATEGY_ID=${sanitize(config.strategyId)}`,
          `FUND_LIMIT_USD=${Math.min(Math.max(config.fundAmountUsd, 10), 1000)}`,
          `RISK_LEVEL=${sanitize(config.riskLevel)}`,
        ],
        expose: [
          { port: 3100, as: 3100, to: [{ global: true }] },
        ],
      },
    },
    profiles: {
      compute: {
        "a2ex-agent": {
          resources: {
            cpu: { units: "0.5" },
            memory: { size: "512Mi" },
            storage: [{ size: "1Gi" }],
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

  return yamlStringify(sdl);
}

function sanitize(input: string): string {
  // Only allow alphanumeric, hyphens, underscores
  return input.replace(/[^a-zA-Z0-9_-]/g, "");
}

function yamlStringify(obj: unknown, indent = 0): string {
  const pad = "  ".repeat(indent);
  if (obj === null || obj === undefined) return "null";
  if (typeof obj === "string") return `"${obj}"`;
  if (typeof obj === "number" || typeof obj === "boolean") return String(obj);
  if (Array.isArray(obj)) {
    if (obj.length === 0) return "[]";
    return obj.map(item => {
      if (typeof item === "object" && item !== null) {
        const lines = yamlStringify(item, indent + 1).split("\n");
        return `${pad}- ${lines[0].trim()}\n${lines.slice(1).join("\n")}`;
      }
      return `${pad}- ${yamlStringify(item)}`;
    }).join("\n");
  }
  if (typeof obj === "object") {
    return Object.entries(obj as Record<string, unknown>)
      .map(([key, val]) => {
        if (typeof val === "object" && val !== null && !Array.isArray(val)) {
          return `${pad}${key}:\n${yamlStringify(val, indent + 1)}`;
        }
        if (Array.isArray(val)) {
          return `${pad}${key}:\n${yamlStringify(val, indent + 1)}`;
        }
        return `${pad}${key}: ${yamlStringify(val)}`;
      })
      .join("\n");
  }
  return String(obj);
}
