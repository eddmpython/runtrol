import type { SessionDescriptor } from "@runtrol/runtime-client";

export function sessionStateLabel(session: Pick<SessionDescriptor, "lifecycle">): string {
  switch (session.lifecycle) {
    case "hotIdle":
      return "idle";
    case "hotRunning":
      return "busy";
    case "failed":
      return "failed";
    case "cold":
      return "detached";
  }
}
