import { describe, it, expect } from "vitest";
import {
  formatQuotaUsd,
  formatRemainingUsd,
  formatShanghaiDateTime,
} from "./accountsTypes";

describe("accountsTypes Quota Semantics & Formatters", () => {
  describe("formatQuotaUsd", () => {
    it("formats raw units at 500,000 raw per USD", () => {
      expect(formatQuotaUsd("500000")).toBe("$1.00");
      expect(formatQuotaUsd("1000000")).toBe("$2.00");
      expect(formatQuotaUsd("250000")).toBe("$0.50");
      expect(formatQuotaUsd("0")).toBe("$0.00");
    });

    it("returns '—' for null, undefined, empty, or negative values without fabricating zero", () => {
      expect(formatQuotaUsd(null)).toBe("—");
      expect(formatQuotaUsd(undefined)).toBe("—");
      expect(formatQuotaUsd("")).toBe("—");
      expect(formatQuotaUsd("-1")).toBe("—");
      expect(formatQuotaUsd("invalid")).toBe("—");
    });
  });

  describe("formatRemainingUsd (New API wallet quota semantics)", () => {
    it("formats quotaRaw directly as remaining wallet balance without double deduction", () => {
      // Official New API: quotaRaw is remaining balance, usedQuotaRaw is independently tracked used quota
      // When quotaRaw is 1,000,000 and usedQuotaRaw is 250,000, remaining reference is $2.00, NOT $1.50
      expect(formatRemainingUsd("1000000", "250000")).toBe("$2.00");
      expect(formatQuotaUsd("250000")).toBe("$0.50");
    });

    it("returns '—' for unknown/null remaining balance without fabricating $0.00", () => {
      expect(formatRemainingUsd(null, "250000")).toBe("—");
      expect(formatRemainingUsd(undefined, "250000")).toBe("—");
      expect(formatRemainingUsd("", "250000")).toBe("—");
      expect(formatRemainingUsd("-500000", "250000")).toBe("—");
    });

    it("returns '$0.00' only when quota is genuinely zero", () => {
      expect(formatRemainingUsd("0", "250000")).toBe("$0.00");
    });
  });

  describe("formatShanghaiDateTime", () => {
    it("formats valid ISO timestamp in CST Asia/Shanghai format", () => {
      const formatted = formatShanghaiDateTime("2026-03-30T04:00:00Z");
      expect(formatted).toContain("2026");
      expect(formatted).toContain("03");
      expect(formatted).toContain("30");
      expect(formatted).toContain("12:00");
    });

    it("returns '-' for missing or invalid timestamps without throwing", () => {
      expect(formatShanghaiDateTime(null)).toBe("-");
      expect(formatShanghaiDateTime(undefined)).toBe("-");
      expect(formatShanghaiDateTime("")).toBe("-");
      expect(formatShanghaiDateTime("not-a-date")).toBe("-");
    });
  });
});
