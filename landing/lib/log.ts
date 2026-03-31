export function log(action: string, data: Record<string, unknown>) {
  console.log(JSON.stringify({ action, ts: new Date().toISOString(), ...data }));
}
