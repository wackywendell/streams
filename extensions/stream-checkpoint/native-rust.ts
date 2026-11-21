import { CardWatchChild, cardPluginRoot } from "./child.ts";
import { projectSessionName } from "./session-naming.ts";
import { sessionKeyOf } from "./streams.ts";
import type { NativeFactory } from "./native.ts";

/**
 * Native frontend bridge:
 * - In `hc` (herdr-app): writes `streams.session` label on `HC_TERMINAL_ID` on start
 *   (full Session key when enrolled, empty string otherwise).
 * - In classic Herdr (`HERDR_ENV=1`): runs `streams card watch pi/<ID>` and forwards
 *   open/close to `streams card open` / `streams card close`.
 * - Projects Pi's session name from the Session title when `pi.naming.session` is enabled.
 */
export const loadRustNative: NativeFactory = async ({
  pi,
  ctx,
  sessionId,
  cwd,
  env,
  cli,
  piNamingSession,
  assertCurrent,
  warning,
}) => {
  let watcher: CardWatchChild | undefined;

  return {
    start: async (signal) => {
      assertCurrent();
      const session = await cli.showSession(sessionId, signal);
      assertCurrent();
      if (env.HC_TERMINAL_ID) {
        try {
          await cli.writeSessionLabel(session ? sessionKeyOf(session) : null, signal);
        } catch (error) {
          warning(error instanceof Error ? error.message : String(error));
        }
      }
      if (session && piNamingSession()) {
        try {
          projectSessionName(pi, ctx.sessionManager, sessionId, session.title.text, assertCurrent);
        } catch (error) {
          try {
            assertCurrent();
            warning(error instanceof Error ? error.message : String(error));
          } catch {
            // Retired UI lifetime
          }
        }
      }
      if (env.HERDR_ENV === "1" && env.HERDR_PANE_ID) {
        watcher = new CardWatchChild({
          sessionId,
          cwd,
          env,
          pluginRoot: cardPluginRoot,
          warning,
        });
        watcher.start();
      }
    },
    open: async (signal) => {
      assertCurrent();
      const result = await cli.cardOpen(sessionId, cardPluginRoot, signal);
      assertCurrent();
      return result;
    },
    close: async (signal) => {
      assertCurrent();
      const result = await cli.cardClose(sessionId, signal);
      assertCurrent();
      return result;
    },
    dispose: async () => {
      await watcher?.stop();
      watcher = undefined;
    },
  };
};
