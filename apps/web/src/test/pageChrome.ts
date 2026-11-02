import { createElement, type ReactNode } from "react";
import { render as renderTestingLibrary, type RenderOptions } from "@testing-library/react";
import { PageChromeFixture } from "./PageChromeFixture";

export function render(ui: ReactNode, options?: RenderOptions) {
  return renderTestingLibrary(ui, { ...options, wrapper: ({ children }) => {
    const Wrapper = options?.wrapper;
    return createElement(PageChromeFixture, null, Wrapper ? createElement(Wrapper, null, children) : children);
  } });
}
