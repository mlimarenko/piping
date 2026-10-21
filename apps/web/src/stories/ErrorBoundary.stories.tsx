import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect } from "react";
import { ErrorBoundary } from "@/shared/ui-shared/ErrorBoundary";

const Bomb = ({ message }: { message: string }) => {
  useEffect(() => {
    throw new Error(message);
  }, [message]);
  return null;
};

const meta: Meta<typeof ErrorBoundary> = {
  title: "App/ErrorBoundary",
  component: ErrorBoundary,
  tags: ["autodocs"],
};

export default meta;

type Story = StoryObj<typeof ErrorBoundary>;

export const Default: Story = {
  render: () => (
    <ErrorBoundary>
      <Bomb message="Network connection lost" />
    </ErrorBoundary>
  ),
};

export const Scoped: Story = {
  render: () => (
    <ErrorBoundary scope="Inventory">
      <Bomb message="Failed to fetch hosts" />
    </ErrorBoundary>
  ),
};

export const HappyPath: Story = {
  render: () => (
    <ErrorBoundary scope="Connections">
      <div className="p-4">
        <h1 className="text-base font-semibold">All good</h1>
        <p className="text-xs text-muted-foreground">No errors thrown.</p>
      </div>
    </ErrorBoundary>
  ),
};
