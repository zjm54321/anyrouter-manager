import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { SettingsPanel } from "./SettingsPanel";
import type { GlobalCheckinConfig } from "./accountsTypes";

const defaultGlobalConfig: GlobalCheckinConfig = {
  enabled: true,
  startTime: "09:00",
  intervalMinutes: 30,
};

describe("SettingsPanel - Gateway Responses Mode", () => {
  it("renders 3 options and warning notice when gatewaySettings is provided", () => {
    render(
      <SettingsPanel
        globalConfig={defaultGlobalConfig}
        busy={false}
        onSaveGlobalCheckin={vi.fn()}
        gatewaySettings={{ responses_mode: "pass" }}
      />
    );

    expect(screen.getByRole("heading", { name: "Responses 转发模式" })).toBeInTheDocument();
    expect(screen.getByText("全部透传（默认）")).toBeInTheDocument();
    expect(screen.getByText("兼容适配（实验性）")).toBeInTheDocument();
    expect(screen.getByText("自动识别（实验性）")).toBeInTheDocument();

    // Exact required warning wording
    expect(
      screen.getByText("实验性：仅补充缺失的缓存标识；通用 UUID 与更多客户端场景的上游兼容性尚未验证。")
    ).toBeInTheDocument();

    // Save button disabled initially because selection matches confirmed server mode
    const saveButton = screen.getByRole("button", { name: "保存设置" });
    expect(saveButton).toBeDisabled();
  });

  it("enables save button on selection change and calls onSaveGatewaySettings with selected mode", async () => {
    const onSave = vi.fn().mockResolvedValue({ responses_mode: "adapt" });
    render(
      <SettingsPanel
        globalConfig={defaultGlobalConfig}
        busy={false}
        onSaveGlobalCheckin={vi.fn()}
        gatewaySettings={{ responses_mode: "pass" }}
        onSaveGatewaySettings={onSave}
      />
    );

    const adaptRadio = screen.getByDisplayValue("adapt");
    fireEvent.click(adaptRadio);

    const saveButton = screen.getByRole("button", { name: "保存设置" });
    expect(saveButton).not.toBeDisabled();

    fireEvent.click(saveButton);
    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave).toHaveBeenCalledWith({ responses_mode: "adapt" });

    // Success feedback
    expect(await screen.findByText("网关设置已保存")).toBeInTheDocument();
  });

  it("does not show success when save resolves with mismatched or invalid confirmed DTO", async () => {
    // Return mismatched mode
    const onSave = vi.fn().mockResolvedValue({ responses_mode: "pass" });
    render(
      <SettingsPanel
        globalConfig={defaultGlobalConfig}
        busy={false}
        onSaveGlobalCheckin={vi.fn()}
        gatewaySettings={{ responses_mode: "pass" }}
        onSaveGatewaySettings={onSave}
      />
    );

    const adaptRadio = screen.getByDisplayValue("adapt");
    fireEvent.click(adaptRadio);

    const saveButton = screen.getByRole("button", { name: "保存设置" });
    fireEvent.click(saveButton);

    expect(onSave).toHaveBeenCalledTimes(1);

    // MUST NOT say "已保存"
    expect(screen.queryByText("网关设置已保存")).not.toBeInTheDocument();
    expect(await screen.findByText("保存失败：服务端确认的设置格式无效或已被更改")).toBeInTheDocument();

    // Draft selection rolls back to confirmed "pass"
    const passRadio = screen.getByDisplayValue("pass") as HTMLInputElement;
    expect(passRadio.checked).toBe(true);
  });

  it("retains last confirmed server mode on save failure (not optimistic success)", async () => {
    const onSave = vi.fn().mockRejectedValue(new Error("网络超时"));
    render(
      <SettingsPanel
        globalConfig={defaultGlobalConfig}
        busy={false}
        onSaveGlobalCheckin={vi.fn()}
        gatewaySettings={{ responses_mode: "pass" }}
        onSaveGatewaySettings={onSave}
      />
    );

    const autoRadio = screen.getByDisplayValue("auto");
    fireEvent.click(autoRadio);

    const saveButton = screen.getByRole("button", { name: "保存设置" });
    fireEvent.click(saveButton);

    expect(await screen.findByText("保存失败：网络超时")).toBeInTheDocument();

    // Reverted to server-confirmed "pass", not remaining optimistic "auto"
    const passRadio = screen.getByDisplayValue("pass") as HTMLInputElement;
    expect(passRadio.checked).toBe(true);
  });

  it("renders explicit error banner and retry button on load failure without confirming pass", () => {
    const onRetry = vi.fn();
    render(
      <SettingsPanel
        globalConfig={defaultGlobalConfig}
        busy={false}
        onSaveGlobalCheckin={vi.fn()}
        gatewaySettings={null}
        gatewaySettingsError="网关设置功能不可用（服务端未启用）"
        onRetryGatewaySettings={onRetry}
      />
    );

    expect(screen.getByText("网关设置功能不可用（服务端未启用）")).toBeInTheDocument();
    // Must NOT display confirmed radio cards or claim pass is active
    expect(screen.queryByDisplayValue("pass")).not.toBeInTheDocument();

    const retryButton = screen.getByRole("button", { name: "重试" });
    fireEvent.click(retryButton);
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("displays loading state when gatewaySettings is null and no error", () => {
    render(
      <SettingsPanel
        globalConfig={defaultGlobalConfig}
        busy={false}
        onSaveGlobalCheckin={vi.fn()}
        gatewaySettings={null}
        gatewaySettingsError={null}
      />
    );

    expect(screen.getByText("正在获取网关转发模式设置…")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("pass")).not.toBeInTheDocument();
  });
});
