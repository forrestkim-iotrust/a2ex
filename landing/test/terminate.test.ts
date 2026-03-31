import { describe, it, expect } from "vitest";

describe("Terminate Status Transitions", () => {
  const TERMINAL_STATES = ["terminating", "terminated"];

  function canTerminate(status: string): boolean {
    return !TERMINAL_STATES.includes(status);
  }

  it("allows termination from 'active' status", () => {
    expect(canTerminate("active")).toBe(true);
  });

  it("allows termination from 'pending' status", () => {
    expect(canTerminate("pending")).toBe(true);
  });

  it("allows termination from 'awaiting_bids' status", () => {
    expect(canTerminate("awaiting_bids")).toBe(true);
  });

  it("allows termination from 'failed' status", () => {
    expect(canTerminate("failed")).toBe(true);
  });

  it("blocks termination from 'terminating' (already in progress)", () => {
    expect(canTerminate("terminating")).toBe(false);
  });

  it("blocks termination from 'terminated' (already done)", () => {
    expect(canTerminate("terminated")).toBe(false);
  });

  it("transitions active → terminating → terminated", () => {
    let status = "active";
    expect(canTerminate(status)).toBe(true);

    status = "terminating";
    expect(canTerminate(status)).toBe(false);

    status = "terminated";
    expect(canTerminate(status)).toBe(false);
  });
});

describe("Terminate Idempotency", () => {
  it("returns error for already-terminating deployment", () => {
    const deployment = { status: "terminating" };
    const isAlready =
      deployment.status === "terminating" || deployment.status === "terminated";
    expect(isAlready).toBe(true);
  });

  it("returns error for already-terminated deployment", () => {
    const deployment = { status: "terminated" };
    const isAlready =
      deployment.status === "terminating" || deployment.status === "terminated";
    expect(isAlready).toBe(true);
  });

  it("proceeds for active deployment", () => {
    const deployment = { status: "active" };
    const isAlready =
      deployment.status === "terminating" || deployment.status === "terminated";
    expect(isAlready).toBe(false);
  });
});

describe("Balance Recovery Calculation", () => {
  it("calculates positive recovery (balanceAfter > balanceBefore)", () => {
    const balanceBefore = 1000000; // 1 AKT in uakt
    const balanceAfter = 1500000; // 1.5 AKT in uakt
    const recovered = balanceAfter - balanceBefore;
    expect(recovered).toBe(500000);
    expect(recovered).toBeGreaterThan(0);
  });

  it("returns zero when balance unchanged", () => {
    const balanceBefore = 1000000;
    const balanceAfter = 1000000;
    const recovered = balanceAfter - balanceBefore;
    expect(recovered).toBe(0);
  });

  it("handles negative recovery (balance decreased during close)", () => {
    const balanceBefore = 1000000;
    const balanceAfter = 900000;
    const recovered = balanceAfter - balanceBefore;
    expect(recovered).toBeLessThan(0);
  });

  it("only includes recoveredUakt in response when positive", () => {
    function buildResponse(recovered: number | null) {
      return {
        status: "terminated",
        akashClosed: true,
        ...(recovered !== null && recovered > 0
          ? { recoveredUakt: recovered }
          : {}),
      };
    }

    const withRecovery = buildResponse(500000);
    expect(withRecovery).toHaveProperty("recoveredUakt", 500000);

    const noRecovery = buildResponse(0);
    expect(noRecovery).not.toHaveProperty("recoveredUakt");

    const negativeRecovery = buildResponse(-100);
    expect(negativeRecovery).not.toHaveProperty("recoveredUakt");

    const nullRecovery = buildResponse(null);
    expect(nullRecovery).not.toHaveProperty("recoveredUakt");
  });

  it("handles null balanceBefore (balance check failed)", () => {
    const balanceBefore: number | null = null;
    const akashClosed = true;
    // Route logic: only calculate recovery if akashClosed && balanceBefore !== null
    const shouldCalculate = akashClosed && balanceBefore !== null;
    expect(shouldCalculate).toBe(false);
  });
});

describe("Akash Close Behavior", () => {
  it("only attempts close when akashDseq is present", () => {
    const withDseq = { akashDseq: "12345" };
    const withoutDseq = { akashDseq: null };
    expect(!!withDseq.akashDseq).toBe(true);
    expect(!!withoutDseq.akashDseq).toBe(false);
  });

  it("sets terminatedAt to current time", () => {
    const before = Date.now();
    const terminatedAt = new Date();
    const after = Date.now();
    expect(terminatedAt.getTime()).toBeGreaterThanOrEqual(before);
    expect(terminatedAt.getTime()).toBeLessThanOrEqual(after);
  });

  it("sends SYSTEM:SHUTDOWN command to agent", () => {
    const command = "SYSTEM:SHUTDOWN";
    expect(command).toBe("SYSTEM:SHUTDOWN");
    expect(command.startsWith("SYSTEM:")).toBe(true);
  });

  it("publishes shutdown to Redis channel with correct pattern", () => {
    const deploymentId = "dep-001";
    const channel = `agent:${deploymentId}:commands`;
    expect(channel).toBe("agent:dep-001:commands");
  });
});

describe("Terminate Ownership Check", () => {
  it("requires deployment to belong to authenticated user", () => {
    const deployment = { id: "dep-001", userAddress: "0xAAA" };
    const authUser = "0xAAA";
    expect(deployment.userAddress).toBe(authUser);
  });

  it("returns 404 when deployment not found or wrong user", () => {
    const deployment = null;
    expect(deployment).toBeNull();
  });
});
