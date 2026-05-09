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
    const tools: Array<{ name: string; execute: Function }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, execute: tool.execute });
      },
    };

    registerTaskerExtension(pi);

    expect(tools.map((tool) => tool.name).sort()).toEqual([
      "tasker_create_child_task",
      "tasker_get_task",
      "tasker_request_transition",
      "tasker_set_acceptance_criterion_status",
      "tasker_set_validation_item_status",
      "tasker_update_workpad",
    ].sort());
  });
});
