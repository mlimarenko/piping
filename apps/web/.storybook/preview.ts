import type { Preview } from "@storybook/react-vite";
import { setupWorker } from "msw/browser";
import { mswLoader } from "msw-storybook-addon/csf3";
import "../src/index.css";

const preview: Preview = {
  parameters: {
    controls: { matchers: { color: /(background|color)$/i, date: /Date$/i } },
    backgrounds: {
      default: "dark",
      values: [
        { name: "dark", value: "#0b0c10" },
        { name: "light", value: "#ffffff" },
      ],
    },
  },
  loaders: [mswLoader(async () => {
    const worker = setupWorker();
    await worker.start({ onUnhandledRequest: "bypass" });
    return worker;
  })],
};

export default preview;
