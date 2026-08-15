import type { SessionDescriptor } from "@runtrol/runtime-client";

export function sessionStateLabel(session: Pick<SessionDescriptor, "lifecycle">): string {
  switch (session.lifecycle) {
    case "hotIdle":
      return "Ready";
    case "hotRunning":
      return "Working";
    case "failed":
      return "Needs attention";
    case "cold":
      return "Saved";
  }
}
