/** Decimal display units only; API and persistence values stay in whole tokens. */
export function validTokenCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

export function tokenCount(value: number | null | undefined) {
  if (!validTokenCount(value)) return "—";
  const whole = Math.floor(value / 1000);
  const remainder = value % 1000;
  const fraction = remainder ? `.${String(remainder).padStart(3, "0").replace(/0+$/, "")}` : "";
  return `${whole}${fraction}k`;
}
