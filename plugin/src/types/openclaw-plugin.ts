/**
 * Minimal type stubs for OpenClaw plugin API.
 *
 * These mirror the shapes from openclaw/src/plugins/types.ts and
 * openclaw/src/agents/tools/common.ts without depending on the full
 * openclaw package. Keeps the plugin self-contained for development.
 *
 * Key insight from research: OpenClawPluginToolContext does NOT have
 * stateDir — only OpenClawPluginServiceContext does. The service start()
 * must capture stateDir into a module-level variable for the tool factory.
 */

import type { JSONSchema7 } from "./json-schema.js";

// --- Agent Tool types ---

/** Minimal AnyAgentTool matching openclaw's AgentTool<any, unknown> */
export interface AnyAgentTool {
  name: string;
  description: string;
  parameters: JSONSchema7;
  execute: (toolCallId: string, params: Record<string, unknown>, signal?: AbortSignal, onUpdate?: unknown) => Promise<unknown>;
  ownerOnly?: boolean;
}

// --- Plugin API types ---

/** Context passed to tool factory on every turn */
export interface OpenClawPluginToolContext {
  /** Note: stateDir is intentionally absent here — it only exists on ServiceContext */
}

/** Context passed to service start() — has stateDir */
export interface OpenClawPluginServiceContext {
  stateDir: string;
  abortSignal?: AbortSignal;
}

/** Service definition registered via api.registerService() */
export interface OpenClawPluginService {
  id: string;
  start: (ctx: OpenClawPluginServiceContext) => void | Promise<void>;
  stop?: () => void | Promise<void>;
}

/** Tool factory signature: returns tools based on current state, or null to expose no tools */
export type OpenClawPluginToolFactory = (
  ctx: OpenClawPluginToolContext,
) => AnyAgentTool | AnyAgentTool[] | null;

/** Options for registerTool (2026.3.13+) */
export interface RegisterToolOpts {
  names?: string[];
  name?: string;
  optional?: boolean;
}

/** The main plugin API passed to register() */
export interface OpenClawPluginApi {
  registerTool: (
    toolOrFactory: AnyAgentTool | AnyAgentTool[] | OpenClawPluginToolFactory,
    opts?: RegisterToolOpts,
  ) => void;
  registerService: (service: OpenClawPluginService) => void;
}
