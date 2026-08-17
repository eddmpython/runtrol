import { RuntimeRequestError } from "@runtrol/runtime-client";

import type { ProviderLine } from "./runtimeTypes";

/// A failure the person has already been shown, together with what to do about it.
///
/// Thrown so callers still unwind, and recognised by the command wrapper so it does not add a second
/// message underneath the first. Without this the operator would see the explanation with its buttons and
/// then a bare protocol string, and the second one would look like a different problem.
export class ServiceTroubleReported extends Error {
  public constructor(message: string) {
    super(message);
    this.name = "ServiceTroubleReported";
  }
}

/// The public error category a failure carries, when it came from the Runtime protocol at all.
///
/// Reads the typed error rather than matching on prose. The message is for a person; the code is the part
/// that is contractually stable, and branching on the other one is how a surface breaks when a sentence is
/// reworded.
export function errorKindOf(error: unknown): string | undefined {
  return error instanceof RuntimeRequestError ? error.failure.code : undefined;
}

/// One thing a person can do right now to get a coding service working.
///
/// The command is the service's own, assembled by Runtime and validated at the manifest boundary to hold
/// no shell separator. Runtrol offers it and never runs it: it goes into the operator's own terminal
/// unexecuted, so they read it before anything happens. An install button that installs is the one
/// capability this product refused from the start, and a friendly label does not change what it is.
export type HelpOffer = {
  /// What the button says.
  readonly label: string;
  /// The exact line placed in the terminal, still waiting for the person to press Enter.
  readonly command: string;
  /// Why this is the thing to try, in one clause.
  readonly because: string;
};

/// Why a coding service could not do what was asked, as the public protocol categorises it.
///
/// Only the categories a person can act on are named. Anything else takes the default order, because
/// guessing wrong about the cause is how a surface sends somebody to sign in when the CLI was not
/// installed at all.
export type ServiceTrouble =
  | "needsSigningIn"
  | "notInstalled"
  | "misbehaving"
  | "unknown";

/// What the public error category means for what to offer.
///
/// `presenceRequired` is the one that carries real information: Runtime says a private local action is
/// required, and for a coding service that is signing in. The rest are read together with what discovery
/// already knows about the installation, because "unavailable" covers both a missing executable and an
/// installed one that will not run.
export function troubleOf(errorKind: string | undefined, provider: ProviderLine | null): ServiceTrouble {
  if (errorKind === "presenceRequired") return "needsSigningIn";
  if (provider?.installation.state === "missing") return "notInstalled";
  if (errorKind === "providerUnavailable" || errorKind === "capabilityUnavailable") {
    return provider?.installation.state === "usable" ? "misbehaving" : "notInstalled";
  }
  return "unknown";
}

/// Everything worth offering for this service, most likely to help first.
///
/// Ordering is the whole value here. A person who is stuck reads the first button, so putting the wrong
/// one there costs them a detour, and offering three equally weighted actions makes them choose between
/// things they have no way to compare.
export function offersFor(provider: ProviderLine, trouble: ServiceTrouble): HelpOffer[] {
  const help = provider.help;
  if (!help) return [];
  const name = provider.displayName;
  const signIn: HelpOffer | null = help.signIn
    ? {
        label: `Sign in to ${name}`,
        command: help.signIn,
        because: "this coding service keeps its own login, and only you can complete it",
      }
    : null;
  const install: HelpOffer | null = help.install
    ? {
        label: `Install ${name}`,
        command: help.install,
        because: "no installed copy of this coding service was found",
      }
    : null;
  const diagnose: HelpOffer | null = help.diagnose
    ? {
        label: `Check ${name}`,
        command: help.diagnose,
        because: "this coding service can diagnose its own installation better than anything else can",
      }
    : null;

  // Each order puts the action that resolves the named trouble first, then the next most plausible. The
  // unknown case leads with the service's own diagnosis rather than with a guess, because that is the one
  // action that is never the wrong thing to have run.
  const ordered = ((): readonly (HelpOffer | null)[] => {
    switch (trouble) {
      case "needsSigningIn":
        return [signIn, diagnose, install];
      case "notInstalled":
        return [install, signIn, diagnose];
      case "misbehaving":
        return [diagnose, signIn, install];
      case "unknown":
        return [diagnose, signIn, install];
    }
  })();
  return ordered.filter((offer): offer is HelpOffer => offer !== null);
}

/// The one action to lead with, or null when this service declared none.
export function firstOffer(provider: ProviderLine, trouble: ServiceTrouble): HelpOffer | null {
  return offersFor(provider, trouble)[0] ?? null;
}

/// The sentence shown above the offers.
///
/// Says what happened in terms of the coding service rather than the transport, because a person reading
/// this is trying to get work done and has no reason to know Runtrol has a protocol.
export function troubleSentence(provider: ProviderLine, trouble: ServiceTrouble): string {
  const name = provider.displayName;
  switch (trouble) {
    case "needsSigningIn":
      return `${name} needs you to sign in to it.`;
    case "notInstalled":
      return `${name} is not installed where Runtrol can run it.`;
    case "misbehaving":
      return `${name} is installed but could not start a conversation.`;
    case "unknown":
      return `${name} could not start a conversation.`;
  }
}
