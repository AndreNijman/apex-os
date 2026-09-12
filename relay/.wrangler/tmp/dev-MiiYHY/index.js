var __defProp = Object.defineProperty;
var __name = (target2, value) => __defProp(target2, "name", { value, configurable: true });

// src/index.js
import { DurableObject } from "cloudflare:workers";

// src/room.js
var NOTICE = {
  waiting: '{"relay":"waiting"}',
  paired: '{"relay":"paired"}',
  peerGone: '{"relay":"peer-gone"}'
};
var ID = /^[A-Za-z0-9_-]{8,128}$/;
function target(pathname, query) {
  const parts = pathname.split("/").filter((p) => p !== "");
  if (parts.length !== 2 || parts[0] !== "r") {
    return { status: 404, why: "not a rendezvous" };
  }
  const rendezvous = parts[1];
  if (!ID.test(rendezvous)) {
    return { status: 404, why: "not a rendezvous" };
  }
  const role = query.get("role");
  if (role !== "host" && role !== "guest") {
    return { status: 400, why: "a connection must say whether it is the host or a guest" };
  }
  return { rendezvous, role };
}
__name(target, "target");
function decide(role, waiting) {
  if (role === "host") {
    if (waiting) {
      return { ok: false, status: 409, why: "this rendezvous already has a host waiting" };
    }
    return { ok: true, act: "hold" };
  }
  if (!waiting) {
    return { ok: false, status: 409, why: "no desktop is waiting at this rendezvous" };
  }
  return { ok: true, act: "join" };
}
__name(decide, "decide");
function refusal(status, why) {
  return { status, body: why + "\n", headers: { "content-type": "text/plain; charset=utf-8" } };
}
__name(refusal, "refusal");

// src/index.js
var src_default = {
  /**
   * @param {Request} request
   * @param {{ROOM: DurableObjectNamespace}} env
   */
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/" || url.pathname === "/health") {
      return new Response("apex-remote-relay\n", {
        headers: { "content-type": "text/plain; charset=utf-8" }
      });
    }
    const asked = target(url.pathname, url.searchParams);
    if ("status" in asked) {
      const r = refusal(asked.status, asked.why);
      return new Response(r.body, { status: r.status, headers: r.headers });
    }
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      const r = refusal(426, "this endpoint speaks WebSocket only");
      return new Response(r.body, { status: r.status, headers: r.headers });
    }
    return env.ROOM.getByName(asked.rendezvous).fetch(request);
  }
};
function isOpen(ws) {
  return ws.readyState === WebSocket.READY_STATE_OPEN;
}
__name(isOpen, "isOpen");
function farewell(ws) {
  if (!isOpen(ws)) {
    return;
  }
  ws.send(NOTICE.peerGone);
  ws.close(1001, "the other end has gone");
}
__name(farewell, "farewell");
var RelayRoom = class extends DurableObject {
  static {
    __name(this, "RelayRoom");
  }
  /**
   * @param {Request} request
   */
  async fetch(request) {
    const url = new URL(request.url);
    const asked = target(url.pathname, url.searchParams);
    if ("status" in asked) {
      const r = refusal(asked.status, asked.why);
      return new Response(r.body, { status: r.status, headers: r.headers });
    }
    const verdict = decide(asked.role, this.waiting() !== null);
    if (!verdict.ok) {
      const r = refusal(verdict.status, verdict.why);
      return new Response(r.body, { status: r.status, headers: r.headers });
    }
    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];
    const id = crypto.randomUUID();
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({ id, role: asked.role, peer: null });
    if (verdict.act === "hold") {
      server.send(NOTICE.waiting);
    } else {
      const host = this.waiting();
      if (host === null) {
        server.close(1013, "try again");
      } else {
        const hostState = host.deserializeAttachment();
        host.serializeAttachment({ ...hostState, peer: id });
        server.serializeAttachment({ id, role: asked.role, peer: hostState.id });
        host.send(NOTICE.paired);
        server.send(NOTICE.paired);
      }
    }
    return new Response(null, { status: 101, webSocket: client });
  }
  /**
   * The live host connection that is holding this room, or null.
   *
   * Derived from the sockets the runtime still has, never from anything
   * written down. A relay that remembered a waiting host whose socket had
   * died would answer that desktop's every later dial with 409, and the
   * machine would be unreachable until its daemon restarted. That is not
   * hypothetical: the local double in `apexd/apex-remoted/tests/relay.rs`
   * made exactly that mistake, and the reconnect suite caught it as "the
   * desktop did not re-arm".
   */
  waiting() {
    for (const ws of this.ctx.getWebSockets()) {
      if (!isOpen(ws)) {
        continue;
      }
      const state = ws.deserializeAttachment();
      if (state && state.role === "host" && state.peer === null) {
        return ws;
      }
    }
    return null;
  }
  /**
   * @param {string} id
   */
  socketFor(id) {
    for (const ws of this.ctx.getWebSockets()) {
      if (!isOpen(ws)) {
        continue;
      }
      const state = ws.deserializeAttachment();
      if (state && state.id === id) {
        return ws;
      }
    }
    return null;
  }
  /**
   * Copy one message to the other end, and look at none of it.
   *
   * @param {WebSocket} ws
   * @param {ArrayBuffer | string} message
   */
  async webSocketMessage(ws, message) {
    const state = ws.deserializeAttachment();
    if (!state || state.peer === null) {
      return;
    }
    const peer = this.socketFor(state.peer);
    if (peer === null) {
      farewell(ws);
      return;
    }
    peer.send(message);
  }
  /**
   * @param {WebSocket} ws
   */
  async webSocketClose(ws) {
    this.partnerLost(ws);
  }
  /**
   * @param {WebSocket} ws
   */
  async webSocketError(ws) {
    this.partnerLost(ws);
  }
  /**
   * @param {WebSocket} ws
   */
  partnerLost(ws) {
    const state = ws.deserializeAttachment();
    if (!state || state.peer === null) {
      return;
    }
    const peer = this.socketFor(state.peer);
    if (peer !== null) {
      farewell(peer);
    }
  }
};

// node_modules/wrangler/templates/middleware/middleware-ensure-req-body-drained.ts
var drainBody = /* @__PURE__ */ __name(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } finally {
    try {
      if (request.body !== null && !request.bodyUsed) {
        const reader = request.body.getReader();
        while (!(await reader.read()).done) {
        }
      }
    } catch (e) {
      console.error("Failed to drain the unused request body.", e);
    }
  }
}, "drainBody");
var middleware_ensure_req_body_drained_default = drainBody;

// node_modules/wrangler/templates/middleware/middleware-miniflare3-json-error.ts
function reduceError(e) {
  return {
    name: e?.name,
    message: e?.message ?? String(e),
    stack: e?.stack,
    cause: e?.cause === void 0 ? void 0 : reduceError(e.cause)
  };
}
__name(reduceError, "reduceError");
var jsonError = /* @__PURE__ */ __name(async (request, env, _ctx, middlewareCtx) => {
  try {
    return await middlewareCtx.next(request, env);
  } catch (e) {
    const error = reduceError(e);
    const body = JSON.stringify(error);
    const headers = {
      "Content-Type": "application/json",
      "MF-Experimental-Error-Stack": "true"
    };
    const encoded = encodeURIComponent(body);
    if (encoded.length <= 8192) {
      headers["MF-Experimental-Error-Stack-Payload"] = encoded;
    }
    return new Response(body, { status: 500, headers });
  }
}, "jsonError");
var middleware_miniflare3_json_error_default = jsonError;

// .wrangler/tmp/bundle-W1AJ6f/middleware-insertion-facade.js
var __INTERNAL_WRANGLER_MIDDLEWARE__ = [
  middleware_ensure_req_body_drained_default,
  middleware_miniflare3_json_error_default
];
var middleware_insertion_facade_default = src_default;

// node_modules/wrangler/templates/middleware/common.ts
var __facade_middleware__ = [];
function __facade_register__(...args) {
  __facade_middleware__.push(...args.flat());
}
__name(__facade_register__, "__facade_register__");
function __facade_invokeChain__(request, env, ctx, dispatch, middlewareChain) {
  const [head, ...tail] = middlewareChain;
  const middlewareCtx = {
    dispatch,
    next(newRequest, newEnv) {
      return __facade_invokeChain__(newRequest, newEnv, ctx, dispatch, tail);
    }
  };
  return head(request, env, ctx, middlewareCtx);
}
__name(__facade_invokeChain__, "__facade_invokeChain__");
function __facade_invoke__(request, env, ctx, dispatch, finalMiddleware) {
  return __facade_invokeChain__(request, env, ctx, dispatch, [
    ...__facade_middleware__,
    finalMiddleware
  ]);
}
__name(__facade_invoke__, "__facade_invoke__");

// .wrangler/tmp/bundle-W1AJ6f/middleware-loader.entry.ts
var __Facade_ScheduledController__ = class ___Facade_ScheduledController__ {
  constructor(scheduledTime, cron, noRetry) {
    this.scheduledTime = scheduledTime;
    this.cron = cron;
    this.#noRetry = noRetry;
  }
  scheduledTime;
  cron;
  static {
    __name(this, "__Facade_ScheduledController__");
  }
  #noRetry;
  noRetry() {
    if (!(this instanceof ___Facade_ScheduledController__)) {
      throw new TypeError("Illegal invocation");
    }
    this.#noRetry();
  }
};
function wrapExportedHandler(worker) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__ === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__.length === 0) {
    return worker;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__) {
    __facade_register__(middleware);
  }
  const fetchDispatcher = /* @__PURE__ */ __name(function(request, env, ctx) {
    if (worker.fetch === void 0) {
      throw new Error("Handler does not export a fetch() function.");
    }
    return worker.fetch(request, env, ctx);
  }, "fetchDispatcher");
  return {
    ...worker,
    fetch(request, env, ctx) {
      const dispatcher = /* @__PURE__ */ __name(function(type, init) {
        if (type === "scheduled" && worker.scheduled !== void 0) {
          const controller = new __Facade_ScheduledController__(
            Date.now(),
            init.cron ?? "",
            () => {
            }
          );
          return worker.scheduled(controller, env, ctx);
        }
      }, "dispatcher");
      return __facade_invoke__(request, env, ctx, dispatcher, fetchDispatcher);
    }
  };
}
__name(wrapExportedHandler, "wrapExportedHandler");
function wrapWorkerEntrypoint(klass) {
  if (__INTERNAL_WRANGLER_MIDDLEWARE__ === void 0 || __INTERNAL_WRANGLER_MIDDLEWARE__.length === 0) {
    return klass;
  }
  for (const middleware of __INTERNAL_WRANGLER_MIDDLEWARE__) {
    __facade_register__(middleware);
  }
  return class extends klass {
    #fetchDispatcher = /* @__PURE__ */ __name((request, env, ctx) => {
      this.env = env;
      this.ctx = ctx;
      if (super.fetch === void 0) {
        throw new Error("Entrypoint class does not define a fetch() function.");
      }
      return super.fetch(request);
    }, "#fetchDispatcher");
    #dispatcher = /* @__PURE__ */ __name((type, init) => {
      if (type === "scheduled" && super.scheduled !== void 0) {
        const controller = new __Facade_ScheduledController__(
          Date.now(),
          init.cron ?? "",
          () => {
          }
        );
        return super.scheduled(controller);
      }
    }, "#dispatcher");
    fetch(request) {
      return __facade_invoke__(
        request,
        this.env,
        this.ctx,
        this.#dispatcher,
        this.#fetchDispatcher
      );
    }
  };
}
__name(wrapWorkerEntrypoint, "wrapWorkerEntrypoint");
var WRAPPED_ENTRY;
if (typeof middleware_insertion_facade_default === "object") {
  WRAPPED_ENTRY = wrapExportedHandler(middleware_insertion_facade_default);
} else if (typeof middleware_insertion_facade_default === "function") {
  WRAPPED_ENTRY = wrapWorkerEntrypoint(middleware_insertion_facade_default);
}
var middleware_loader_entry_default = WRAPPED_ENTRY;
export {
  RelayRoom,
  __INTERNAL_WRANGLER_MIDDLEWARE__,
  middleware_loader_entry_default as default
};
//# sourceMappingURL=index.js.map
