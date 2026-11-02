import type { Preview } from "@storybook/react-vite";
import { initialize, mswLoader } from "msw-storybook-addon";
import "../src/index.css";

initialize({ onUnhandledRequest: "bypass" });

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
  loaders: [mswLoader],
};

export default preview;
