import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import registerSpoolExtension from "../src/index";
import type { ExtensionAPI } from "../src/types";

const originalEnv = { ...process.env };
const originalFetch = globalThis.fetch;
const requests: Array<{ url: string; init: RequestInit }> = [];

beforeEach(() => {
  requests.length = 0;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    requests.push({ url: String(url), init: init ?? {} });
    return new Response(JSON.stringify({ task: { identifier: "TASKER-999" } }), {
      status: 201,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;
});

afterEach(() => {
  process.env = { ...originalEnv };
  globalThis.fetch = originalFetch;
});

describe("registerSpoolExtension", () => {
  it("registers the minimal Spool tool set", () => {
    process.env.SPOOL_API_TOKEN = "token";
    const tools: Array<{ name: string; parameters: any; execute: Function }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, parameters: tool.parameters, execute: tool.execute });
      },
    };

    registerSpoolExtension(pi);

    expect(tools.map((tool) => tool.name).sort()).toEqual([
      "spool_append_workpad",
      "spool_attach_task_link",
      "spool_create_child_task",
      "spool_create_delegated_root_task",
      "spool_get_task",
      "spool_get_task_context_bundle",
      "spool_record_review_decision",
      "spool_report_worker_status",
      "spool_request_transition",
      "spool_refine_backlog_task",
      "spool_set_acceptance_criterion_status",
      "spool_set_validation_item_status",
      "spool_update_workpad",
    ].sort());
  });

  it("constrains status and transition parameters", () => {
    process.env.SPOOL_API_TOKEN = "token";
    const tools: Array<{ name: string; parameters: any }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, parameters: tool.parameters });
      },
    };

    registerSpoolExtension(pi);

    const byName = Object.fromEntries(tools.map((tool) => [tool.name, tool.parameters]));
    expect(byName.spool_get_task_context_bundle.properties.identifier.description).toBe("Task Identifier, such as TASKER-1");
    expect(Object.keys(byName.spool_attach_task_link.properties).sort()).toEqual([
      "identifier",
      "is_primary",
      "kind",
      "label",
      "target",
    ].sort());
    expect(byName.spool_attach_task_link.properties.is_primary.type).toBe("boolean");
    expect(byName.spool_set_acceptance_criterion_status.properties.status.anyOf.map((item: any) => item.const)).toEqual([
      "pending",
      "satisfied",
      "waived",
    ]);
    expect(byName.spool_set_validation_item_status.properties.status.anyOf.map((item: any) => item.const)).toEqual([
      "pending",
      "passed",
      "failed",
      "waived",
    ]);
    expect(byName.spool_request_transition.properties.to_state.anyOf.map((item: any) => item.const)).toContain(
      "integrating",
    );
    expect(byName.spool_record_review_decision.properties.decision.anyOf.map((item: any) => item.const)).toEqual([
      "approve",
      "rework",
    ]);
    expect(byName.spool_create_delegated_root_task.properties.initial_state.anyOf.map((item: any) => item.const)).toEqual([
      "backlog",
      "ready",
    ]);
    expect(byName.spool_refine_backlog_task.properties.target_state.anyOf.map((item: any) => item.const)).toEqual([
      "backlog",
      "ready",
    ]);
    expect(byName.spool_report_worker_status.properties.status.anyOf.map((item: any) => item.const)).toEqual([
      "completion_intent",
      "blocked",
      "retryable_failure",
    ]);
  });

  it("executes delegated Root Task creation through the extension tool with a Delegating Agent actor", async () => {
    process.env.SPOOL_API_URL = "http://tasker.test";
    process.env.SPOOL_API_TOKEN = "token";
    process.env.SPOOL_ACTOR_KIND = "delegating_agent";
    process.env.SPOOL_ACTOR_ID = "delegate-session";
    process.env.SPOOL_ACTOR_DISPLAY_NAME = "Delegation Session";
    const tools: Array<{ name: string; parameters: any; execute: Function }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, parameters: tool.parameters, execute: tool.execute });
      },
    };

    registerSpoolExtension(pi);
    const createTool = tools.find((tool) => tool.name === "spool_create_delegated_root_task");
    expect(createTool).toBeDefined();

    const result = await createTool!.execute("tool-1", {
      queue_key: "TASKER",
      title: "Delegate through extension",
      brief: "Task Brief from a human-present pi session.",
      priority: "urgent",
      initial_state: "ready",
      review_required: false,
      tags: ["dogfood"],
      conflict_hints: ["extensions/spool-pi"],
      blocking_task_identifiers: [],
      acceptance_criteria: ["The Task is created through the Spool Pi Extension."],
      validation_items: ["Fake-extension test observes the delegated-root API call."],
    }, new AbortController().signal);

    expect(result.details).toEqual({ task: { identifier: "TASKER-999" } });
    expect(requests[0].url).toBe("http://tasker.test/tasks/delegated-root");
    expect(requests[0].init.method).toBe("POST");
    expect((requests[0].init.headers as Record<string, string>).authorization).toBe("Bearer token");
    expect(JSON.parse(requests[0].init.body as string)).toEqual({
      actor: {
        kind: "delegating_agent",
        id: "delegate-session",
        display_name: "Delegation Session",
      },
      draft: {
        queue_key: "TASKER",
        title: "Delegate through extension",
        brief: "Task Brief from a human-present pi session.",
        priority: "urgent",
        initial_state: "ready",
        review_required: false,
        tags: ["dogfood"],
        conflict_hints: ["extensions/spool-pi"],
        blocking_task_identifiers: [],
        acceptance_criteria: ["The Task is created through the Spool Pi Extension."],
        validation_items: ["Fake-extension test observes the delegated-root API call."],
      },
    });
  });

  it("executes Task Link attachment through the extension tool with the configured actor", async () => {
    process.env.SPOOL_API_URL = "http://tasker.test";
    process.env.SPOOL_API_TOKEN = "token";
    process.env.SPOOL_ACTOR_ID = "worker-1";
    process.env.SPOOL_ACTOR_DISPLAY_NAME = "Worker One";
    const tools: Array<{ name: string; parameters: any; execute: Function }> = [];
    const pi: ExtensionAPI = {
      registerTool(tool) {
        tools.push({ name: tool.name, parameters: tool.parameters, execute: tool.execute });
      },
    };

    registerSpoolExtension(pi);
    const linkTool = tools.find((tool) => tool.name === "spool_attach_task_link");
    expect(linkTool).toBeDefined();

    const result = await linkTool!.execute("tool-1", {
      identifier: "TASKER-999",
      kind: "review_artifact",
      target: "file:///tmp/review.md",
      label: "Review artifact",
      is_primary: true,
    }, new AbortController().signal);

    expect(result.details).toEqual({ task: { identifier: "TASKER-999" } });
    expect(requests[0].url).toBe("http://tasker.test/tasks/TASKER-999/links");
    expect(requests[0].init.method).toBe("POST");
    expect(JSON.parse(requests[0].init.body as string)).toEqual({
      actor: {
        kind: "worker_agent",
        id: "worker-1",
        display_name: "Worker One",
      },
      link: {
        kind: "review_artifact",
        target: "file:///tmp/review.md",
        label: "Review artifact",
        is_primary: true,
      },
    });
  });
});
