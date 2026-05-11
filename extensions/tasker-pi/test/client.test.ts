import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { TaskerClient, configFromEnv } from "../src/client";

const originalFetch = globalThis.fetch;
const requests: Array<{ url: string; init: RequestInit }> = [];

beforeEach(() => {
  requests.length = 0;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    requests.push({ url: String(url), init: init ?? {} });
    const body = String(url).endsWith("/tasks/TASK-1/context-bundle")
      ? { task: { task: { identifier: "TASK-1" } }, queue: { key: "TASK" }, local_workflow: {}, agent_runs: [] }
      : String(url).endsWith("/tasks/TASK-1")
        ? { workpad_note: { body: "existing notes" } }
        : { ok: true };
    return new Response(JSON.stringify(body), {
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

  it("fetches the task context bundle from the narrow run-start endpoint", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.getTaskContextBundle("TASK-1");

    expect(requests[0].url).toBe("http://tasker.test/tasks/TASK-1/context-bundle");
    expect(requests[0].init.method).toBe("GET");
  });

  it("rejects context bundles with raw transcript or launcher payload fields", async () => {
    globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
      requests.push({ url: String(url), init: init ?? {} });
      return new Response(JSON.stringify({ task: {}, queue: {}, local_workflow: {}, agent_runs: [], raw_json: "{}" }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }) as typeof fetch;
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await expect(client.getTaskContextBundle("TASK-1")).rejects.toThrow("forbidden field raw_json");
  });

  it("updates workpad with actor", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.updateWorkpad("TASK-1", actor, "notes");

    expect(requests[0].init.method).toBe("PUT");
    expect(JSON.parse(requests[0].init.body as string)).toEqual({ actor, body: "notes" });
  });

  it("appends workpad text by fetching the current note before updating", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.appendWorkpad("TASK-1", actor, "new notes");

    expect(requests.map((request) => request.init.method)).toEqual(["GET", "PUT"]);
    expect(JSON.parse(requests[1].init.body as string)).toEqual({ actor, body: "existing notes\n\nnew notes" });
  });

  it("appends workpad text without a leading separator when no note exists", async () => {
    globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
      requests.push({ url: String(url), init: init ?? {} });
      const body = String(url).endsWith("/tasks/TASK-1") ? { workpad_note: null } : { ok: true };
      return new Response(JSON.stringify(body), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }) as typeof fetch;
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.appendWorkpad("TASK-1", actor, "new notes");

    expect(JSON.parse(requests[1].init.body as string)).toEqual({ actor, body: "new notes" });
  });

  it("sends validated base commit when setting validation status", async () => {
    const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

    await client.setValidationItemStatus({
      identifier: "TASK-1",
      position: 1,
      status: "passed",
      validated_base_commit: "abc123",
    }, actor);

    expect(requests[0].url).toBe("http://tasker.test/tasks/TASK-1/validation-items/1/status");
    expect(JSON.parse(requests[0].init.body as string)).toEqual({
      actor,
      status: "passed",
      waiver_reason: null,
      validated_base_commit: "abc123",
    });
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

  it("writes supervisor-readable worker status reports", () => {
    const dir = mkdtempSync(join(tmpdir(), "tasker-status-"));
    try {
      const path = join(dir, "worker.jsonl");
      const client = new TaskerClient({ apiUrl: "http://tasker.test", apiToken: "token" });

      const report = client.reportWorkerStatus(
        { identifier: "TASK-1", status: "completion_intent", message: "handed off", agent_run_id: "run-1" },
        actor,
        path,
      ) as any;

      expect(report.tasker_worker_status).toBe(true);
      const line = JSON.parse(readFileSync(path, "utf8"));
      expect(line).toMatchObject({
        tasker_worker_status: true,
        task_identifier: "TASK-1",
        agent_run_id: "run-1",
        status: "completion_intent",
        message: "handed off",
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
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

  it("captures the supervisor worker status path", () => {
    const config = configFromEnv({
      TASKER_API_TOKEN: "token",
      TASKER_WORKER_STATUS_PATH: "/tmp/status.jsonl",
    });

    expect(config.workerStatusPath).toBe("/tmp/status.jsonl");
  });
});
