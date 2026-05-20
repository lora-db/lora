import { describe, expect, it, vi } from "vitest";
import { createRef } from "react";
import { act, render } from "@testing-library/react";
import {
  LoraQueryEditor,
  type LoraQueryEditorHandle,
} from "../src/LoraQueryEditor";
import { darkTheme } from "../src/themes";

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe("LoraQueryEditor (React)", () => {
  it("mounts and renders the source through CodeMirror", () => {
    const { container } = render(
      <LoraQueryEditor value="MATCH (n) RETURN n" />,
    );
    expect(container.querySelector(".cm-editor")).toBeTruthy();
    expect(container.textContent).toContain("MATCH");
  });

  it("exposes prettify/getValue/setValue via the imperative handle", async () => {
    const ref = createRef<LoraQueryEditorHandle>();
    render(
      <LoraQueryEditor ref={ref} value="match (n) where n.age > 18 return n" />,
    );
    await act(async () => {
      await ref.current?.prettify();
    });
    const value = ref.current?.getValue() ?? "";
    expect(value).toContain("MATCH (n)");
    expect(value).toContain("WHERE n.age > 18");
    expect(value).toContain("RETURN n");

    await act(async () => {
      ref.current?.setValue("CREATE (a:Thing)");
    });
    expect(ref.current?.getValue()).toBe("CREATE (a:Thing)");
  });

  it("fires onRun on Mod-Enter via the imperative handle", async () => {
    const onRun = vi.fn();
    const ref = createRef<LoraQueryEditorHandle>();
    render(
      <LoraQueryEditor ref={ref} value="MATCH (n) RETURN n" onRun={onRun} />,
    );
    await act(async () => {
      ref.current?.run();
    });
    expect(onRun).toHaveBeenCalledWith("MATCH (n) RETURN n");
  });

  it("applies CSS variables from the theme prop to the container", () => {
    const { container } = render(
      <LoraQueryEditor value="MATCH (n) RETURN n" theme={darkTheme} />,
    );
    const root = container.querySelector(".lora-query") as HTMLElement;
    expect(root.style.getPropertyValue("--lq-bg")).toBe(darkTheme.background);
    expect(root.style.getPropertyValue("--lq-color-keyword")).toBe(
      darkTheme.keyword,
    );
  });

  it("invokes onDiagnostics with semantic + syntax results", async () => {
    const onDiagnostics = vi.fn();
    render(
      <LoraQueryEditor
        value="MATCH (n) WHERE a.name = 'incomplete"
        onDiagnostics={onDiagnostics}
      />,
    );
    // Wait one tick so the async validator can resolve.
    await act(async () => {
      await wait(50);
    });
    expect(onDiagnostics).toHaveBeenCalled();
    const lastCall = onDiagnostics.mock.calls.at(-1)![0];
    expect(lastCall.length).toBeGreaterThan(0);
    expect(lastCall[0].severity).toBe("error");
  });

  it("getParameters returns $param references", async () => {
    const ref = createRef<LoraQueryEditorHandle>();
    render(
      <LoraQueryEditor
        ref={ref}
        value="MATCH (n) WHERE n.id = $userId RETURN n LIMIT $cap"
      />,
    );
    const params = await ref.current?.getParameters();
    expect(params).toEqual(expect.arrayContaining(["userId", "cap"]));
  });
});
