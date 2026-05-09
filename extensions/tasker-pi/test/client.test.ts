import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { TaskerClient, configFromEnv } from "../src/client";

const originalFetch = globalThis.fetch;
const requests: Array<{ url: string; init: RequestInit }> = [];

beforeEach(() => {
  requests.length = 0;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    requests.push({ url: String(url), init: init ?? {} });
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("TaskerClient", () => {
  const actor = { kind: "worker_agent", id: "worker", display_name: "Worker" };

  it("sends auth header and fetches a task", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test/", apiToken: "token" });

    await client.getTask("TASK-1");

    expect(requests[0].url).toBe("http://tasker.test/tasks/TASK-1");
    expect(requests[0].init.method).toBe("GET");
    expect((requests[0].init.headers as Record<string, string>).authorization).toBe("Bearer token");
  });

  it("updates workpad with actor", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.updateWorkpad("TASK-1", actor, "notes");

    expect(requests[0].init.method).toBe("PUT");
    expect(JSON.parse(requests[0].init.body as string)).toEqual({ actor, body: "notes" });
  });

  it("creates child tasks through the parent task endpoint", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.createChildTask({
      parent_identifier: "TASK-1",
      title: "Child",
      brief: "Do child work",
      acceptance_criteria: ["works"],
      validation_items: ["tests"],
    }, actor);

    expect(requests[0].url).toBe("http://tasker.test/tasks/TASK-1/child-tasks");
    const body = JSON.parse(requests[0].init.body as string);
    expect(body.actor).toEqual(actor);
    expect(body.task.state).toBe("backlog");
    expect(body.task.blocks_parent).toBe(false);
  });

  it("requests transitions with an agent run id", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.requestTransition("TASK-1", "integrating", actor, "run-1");

    expect(requests[0].url).toBe("http://tasker.test/tasks/TASK-1/transition");
    expect(JSON.parse(requests[0].init.body as string)).toEqual({
      actor,
      to_state: "integrating",
      agent_run_id: "run-1",
    });
  });
});

describe("configFromEnv", () => {
  it("requires token", () => {
    expect(() => configFromEnv({})).toThrow("TASKER_API_TOKEN");
  });

  it("builds worker actor config", () => {
    const config = configFromEnv({
      TASKER_API_URL: "http://localhost:9999",
      TASKER_API_TOKEN: "token",
      TASKER_ACTOR_ID: "worker-1",
      TASKER_AGENT_RUN_ID: "run-1",
    });

    expect(config.actor.kind).toBe("worker_agent");
    expect(config.actor.id).toBe("worker-1");
    expect(config.agentRunId).toBe("run-1");
  });
});
