import { afterEach, describe, expect, it } from "bun:test";
import registerTaskerExtension from "../src/index";
import type { ExtensionAPI } from "../src/types";

const originalEnv = { ...process.env };

afterEach(() => {
  process.env = { ...originalEnv };
});

describe("registerTaskerExtension", () => {
  it("registers the minimal Tasker tool set", () => {
    process.env.TASKER_API_TOKEN = "token";
    const tools: Array<{ name: string; parameters: any; execute: Function }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, parameters: tool.parameters, execute: tool.execute });
      },
    };

    registerTaskerExtension(pi);

    expect(tools.map((tool) => tool.name).sort()).toEqual([
      "tasker_append_workpad",
      "tasker_create_child_task",
      "tasker_get_task",
      "tasker_request_transition",
      "tasker_set_acceptance_criterion_status",
      "tasker_set_validation_item_status",
      "tasker_update_workpad",
    ].sort());
  });

  it("constrains status and transition parameters", () => {
    process.env.TASKER_API_TOKEN = "token";
    const tools: Array<{ name: string; parameters: any }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, parameters: tool.parameters });
      },
    };

    registerTaskerExtension(pi);

    const byName = Object.fromEntries(tools.map((tool) => [tool.name, tool.parameters]));
    expect(byName.tasker_set_acceptance_criterion_status.properties.status.anyOf.map((item: any) => item.const)).toEqual([
      "pending",
      "satisfied",
      "waived",
    ]);
    expect(byName.tasker_set_validation_item_status.properties.status.anyOf.map((item: any) => item.const)).toEqual([
      "pending",
      "passed",
      "failed",
      "waived",
    ]);
    expect(byName.tasker_request_transition.properties.to_state.anyOf.map((item: any) => item.const)).toContain(
      "integrating",
    );
  });
});
