import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";

import { Modal } from "./Modal";

const backdropClass = "fixed inset-0 flex items-center justify-center";

describe("Modal 弹层模板", () => {
  it("渲染 children（面板元素原样保留）", () => {
    render(
      <Modal className={backdropClass} data-testid="backdrop">
        <div className="nm-card" data-testid="panel">
          内容
        </div>
      </Modal>,
    );
    expect(screen.getByTestId("panel").textContent).toBe("内容");
    // rest 透传：backdrop 上的 testid/aria 由 {...rest} 保留
    expect(screen.getByTestId("backdrop")).toBeTruthy();
  });

  it("backdrop pointerdown 调 onClose", () => {
    const onClose = vi.fn();
    render(
      <Modal className={backdropClass} onClose={onClose} data-testid="backdrop">
        <div className="nm-card">面板</div>
      </Modal>,
    );
    fireEvent.pointerDown(screen.getByTestId("backdrop"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("面板内 pointerdown 不调 onClose（stopPropagation 注入生效）", () => {
    const onClose = vi.fn();
    render(
      <Modal className={backdropClass} onClose={onClose} data-testid="backdrop">
        <div className="nm-card" data-testid="panel">
          面板
        </div>
      </Modal>,
    );
    fireEvent.pointerDown(screen.getByTestId("panel"));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closeOnOutside=false：backdrop pointerdown 不调 onClose", () => {
    const onClose = vi.fn();
    render(
      <Modal
        className={backdropClass}
        onClose={onClose}
        closeOnOutside={false}
        data-testid="backdrop"
      >
        <div className="nm-card">面板</div>
      </Modal>,
    );
    fireEvent.pointerDown(screen.getByTestId("backdrop"));
    expect(onClose).not.toHaveBeenCalled();
  });

  it("closeOnEsc：Escape 调 onClose；未开时不调", () => {
    const onEscClose = vi.fn();
    const { rerender } = render(
      <Modal
        className={backdropClass}
        onClose={onEscClose}
        closeOnEsc
        data-testid="backdrop"
      >
        <div className="nm-card">面板</div>
      </Modal>,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onEscClose).toHaveBeenCalledTimes(1);

    onEscClose.mockClear();
    rerender(
      <Modal className={backdropClass} onClose={onEscClose} data-testid="b2">
        <div className="nm-card">面板</div>
      </Modal>,
    );
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onEscClose).not.toHaveBeenCalled();
  });

  it("portal=true 渲染进 document.body", () => {
    render(
      <Modal
        className={backdropClass}
        portal
        onClose={() => {}}
        data-testid="backdrop"
      >
        <div className="nm-card" data-testid="panel">
          面板
        </div>
      </Modal>,
    );
    const backdrop = screen.getByTestId("backdrop");
    expect(backdrop.parentElement).toBe(document.body);
    expect(screen.getByTestId("panel").textContent).toBe("面板");
  });
});
