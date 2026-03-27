import { neon } from "@neondatabase/serverless";
import { drizzle } from "drizzle-orm/neon-http";
import { strategies } from "./schema";
import { strategySeedData } from "./strategies-seed";

async function seed() {
  const sql = neon(process.env.NEON_DATABASE_URL!);
  const db = drizzle(sql);

  console.log("Seeding strategies...");
  for (const s of strategySeedData) {
    await db.insert(strategies).values(s).onConflictDoNothing();
  }
  console.log("Done. 3 strategies seeded.");
}

seed().catch(console.error);
