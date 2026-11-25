import { randomUUID } from "node:crypto";
import {
  createStationConnection,
  type Lease,
  LeaseHeldElsewhereError,
} from "hebbian-rua";
import { Hono } from "hono";
import { streamSSE } from "hono/streaming";
import {
  manualRequest,
  type ViewerEvent,
} from "../../shared/manual-session.js";

interface ManualSession {
  connection: ReturnType<typeof createStationConnection>;
  grant: { id: string; lease: Lease } | null;
  lastHeartbeat: number;
}

export function manualSessions(stationUrl: string) {
  const sessions = new Map<string, ManualSession>();
  const app = new Hono();
  app.get("/events", (context) =>
    streamSSE(context, async (stream) => {
      const id = randomUUID();
      const connection = createStationConnection(stationUrl, "manual");
      const session: ManualSession = {
        connection,
        grant: null,
        lastHeartbeat: Date.now(),
      };
      sessions.set(id, session);
      // WHY: Coalesce snapshots when the tab reads more slowly than the station produces them.
      const pending = new Map<ViewerEvent["kind"], ViewerEvent>();
      let wake = () => {};
      let closed = false;
      function publish(event: ViewerEvent) {
        pending.set(event.kind, event);
        wake();
      }
      function close() {
        if (closed) return;
        closed = true;
        sessions.delete(id);
        connection.close();
        wake();
      }
      stream.onAbort(close);
      connection.onState((state) => publish({ kind: "state", state }));
      connection.onRun((run) => publish({ kind: "run", run }));
      connection.onActionSequence((sequence) =>
        publish({ kind: "sequence", sequence }),
      );
      connection.onConnectionChange((connected) =>
        publish({ kind: "connected", connected }),
      );
      connection.onLeaseLost(() => {
        if (session.grant)
          publish({ kind: "leaseLost", leaseId: session.grant.id });
        session.grant = null;
      });
      const timer = setInterval(() => {
        if (Date.now() - session.lastHeartbeat > 12000) {
          close();
          stream.abort();
        } else publish({ kind: "heartbeat" });
      }, 3000);
      try {
        publish({ kind: "session", id });
        connection.start();
        while (!closed) {
          if (!pending.size)
            await new Promise<void>((resolve) => {
              wake = resolve;
            });
          const batch = [...pending.values()];
          pending.clear();
          for (const event of batch)
            await stream.writeSSE({ data: JSON.stringify(event) });
        }
      } finally {
        clearInterval(timer);
        close();
      }
    }),
  );
  app.post("/request", async (context) => {
    const sessionId = context.req.header("x-viewer-session") ?? "";
    const parsed = manualRequest.safeParse(
      await context.req.json().catch(() => null),
    );
    const session = sessions.get(sessionId);
    if (!session) return context.json({ detail: "Viewer session ended" }, 409);
    if (!parsed.success)
      return context.json({ detail: "Invalid viewer request" }, 400);
    const request = parsed.data;
    const connection = session.connection;
    switch (request.kind) {
      case "heartbeat":
        session.lastHeartbeat = Date.now();
        return context.body(null, 204);
      case "preview":
        return context.json(
          await connection.preview(request.arm, request.pose),
        );
      case "power":
        return context.json(
          await connection
            .arm(request.arm, {
              waitForOutcome: request.waitForOutcome,
              signal: context.req.raw.signal,
            })
            [request.action](),
        );
      case "acquire": {
        try {
          const lease = await connection.acquireLease(request.label);
          if (!sessions.has(sessionId) || !lease.isHeld()) {
            if (lease.isHeld()) await lease.return();
            return context.json(
              { detail: "Viewer session ended during acquisition" },
              409,
            );
          }
          const id = randomUUID();
          session.grant = { id, lease };
          return context.json({
            leaseId: id,
            leaseSeconds: lease.leaseSeconds,
          });
        } catch (error) {
          if (error instanceof LeaseHeldElsewhereError)
            return context.json(error.answer, 409);
          throw error;
        }
      }
    }
    const grant = session.grant;
    if (!grant || grant.id !== request.leaseId || !grant.lease.isHeld())
      return context.json({ detail: "Manual lease ended" }, 409);
    switch (request.kind) {
      case "return":
        await grant.lease.return();
        return context.body(null, 204);
      case "cancel":
        await grant.lease.cancelOperation(request.operationId);
        return context.body(null, 204);
      case "goal":
        return context.json(
          await grant.lease.updateStreamGoal(
            request.operationId,
            request.goal,
            context.req.raw.signal,
          ),
        );
      case "rehome":
        return context.json(
          await grant.lease.runCommand(
            request.arm,
            { kind: "rehome" },
            undefined,
            context.req.raw.signal,
            undefined,
            request.operationId,
          ),
        );
      case "send":
        return context.json(
          await grant.lease.sendCommand(
            request.arm,
            request.command,
            undefined,
            undefined,
            context.req.raw.signal,
            request.operationId,
          ),
        );
    }
  });
  return app;
}
