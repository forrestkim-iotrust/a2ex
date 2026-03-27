import { Redis } from "@upstash/redis";

let redis: Redis | null = null;

export function getRedis(): Redis {
  if (!redis) {
    // Parse redis:// URL to extract host and token for REST API
    const url = process.env.UPSTASH_REDIS_URL!;
    // Upstash REST API needs HTTPS URL and token
    // Extract from redis://default:TOKEN@HOST:PORT
    const match = url.match(/redis:\/\/default:([^@]+)@([^:]+)/);
    if (!match) throw new Error("Invalid UPSTASH_REDIS_URL format");
    const [, token, host] = match;
    redis = new Redis({
      url: `https://${host}`,
      token,
    });
  }
  return redis;
}
