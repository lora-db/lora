"use client";

/**
 * Drives the StatusBar's "DB ready" indicator by lazily booting the
 * WASM database and reporting node/relationship counts. The hook
 * mounts on the client only — `getDb()` throws on the server.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import {
  getDb,
  nodeCount,
  relationshipCount,
  resetDbBoot,
} from "@/lib/db/client";
import { LORADB_MUTATION_EVENT } from "@/lib/actions/runActiveTab";

export type DbState = "idle" | "booting" | "ready" | "error";

export interface DbStatus {
  state: DbState;
  error?: string;
  nodes: number;
  rels: number;
  refresh: () => Promise<void>;
  /**
   * Drop the cached boot promise and try again. Cheap to call: if the
   * previous run already succeeded this is a no-op, otherwise the next
   * `getDb()` will reimport the WASM module from scratch.
   */
  retry: () => Promise<void>;
}

export function useDbStatus(): DbStatus {
  const [state, setState] = useState<DbState>("idle");
  const [error, setError] = useState<string | undefined>(undefined);
  const [nodes, setNodes] = useState(0);
  const [rels, setRels] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const mountedRef = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const [n, r] = await Promise.all([nodeCount(), relationshipCount()]);
      if (!mountedRef.current) return;
      setNodes(n);
      setRels(r);
    } catch (err) {
      if (!mountedRef.current) return;
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  const retry = useCallback(async (): Promise<void> => {
    resetDbBoot();
    setAttempt((n) => n + 1);
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    setState("booting");
    setError(undefined);

    (async () => {
      try {
        await getDb();
        if (!mountedRef.current) return;
        setState("ready");
        await refresh();
      } catch (err) {
        if (!mountedRef.current) return;
        setError(err instanceof Error ? err.message : String(err));
        setState("error");
      }
    })().catch(() => {
      // Promise above already swallows; this catch is belt-and-braces.
    });

    return () => {
      mountedRef.current = false;
    };
    // Re-run when the caller explicitly retries via `retry()`.
  }, [refresh, attempt]);

  // After any mutating run, `runActiveTab` posts `loradb:mutation` so
  // the status counts stay in sync without each consumer wiring its
  // own refresh path.
  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const onMutation = () => {
      void refresh();
    };
    window.addEventListener(LORADB_MUTATION_EVENT, onMutation);
    return () => {
      window.removeEventListener(LORADB_MUTATION_EVENT, onMutation);
    };
  }, [refresh]);

  return { state, error, nodes, rels, refresh, retry };
}
