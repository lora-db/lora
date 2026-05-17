"use client";

/**
 * "Run" button. Mirrors the active-tab's run state so the user gets
 * immediate feedback while the WASM query is in flight.
 */

import { Button, Loader, Tooltip } from "@mantine/core";
import { IconPlayerPlayFilled } from "@tabler/icons-react";

import { useActiveResult, useActiveTab } from "@/lib/state/selectors";
import { runActiveTab } from "@/lib/actions/runActiveTab";

export function RunButton() {
  const result = useActiveResult();
  const tab = useActiveTab();
  const isRunning = result?.state === "running";
  const hasBody = tab !== null && tab.body.trim().length > 0;
  const disabled = isRunning || !hasBody;

  return (
    <Tooltip label="Run query" withArrow>
      <Button
        size="xs"
        color={disabled ? "gray" : "green"}
        leftSection={
          isRunning ? <Loader size={14} color="white" /> : <IconPlayerPlayFilled size={14} />
        }
        disabled={disabled}
        onClick={() => {
          void runActiveTab();
        }}
        aria-label="Run query"
      >
        {isRunning ? "Running" : "Run"}
      </Button>
    </Tooltip>
  );
}
