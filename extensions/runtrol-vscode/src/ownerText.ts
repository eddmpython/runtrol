import { PUBLIC_LIMITS, RuntimeRequestError, type WindowInputBinding, type WindowInputClaim, type WindowInputOfferedNotification, type WindowInputReceiptParams } from "@runtrol/runtime-client";

export type TextTerminal = { sendText(text: string, shouldExecute: boolean): void };
export type OwnerReceipt = Omit<WindowInputReceiptParams, "subscriptionId">;

export type TextOwnerPort = {
  current(binding: WindowInputBinding): TextTerminal | null;
  authorize(sequence: number): Promise<WindowInputClaim>;
  complete(receipt: OwnerReceipt): Promise<void>;
};

/** One receiver for one registration. It retains sequence facts, never historical input. */
export class OwnerText {
  private sequence = 0;
  private active = false;
  private closed = false;

  constructor(private readonly port: TextOwnerPort) {}

  close(): void { this.closed = true; }

  async receive(offer: WindowInputOfferedNotification): Promise<void> {
    if (!Number.isSafeInteger(offer.sequence) || offer.sequence <= this.sequence) {
      throw new Error("the owner delivery sequence is stale");
    }
    if (this.closed || this.active) throw new Error("the owner input receiver is unavailable");
    this.sequence = offer.sequence;
    this.active = true;
    let receipt: OwnerReceipt;
    try {
      const target = this.port.current(offer.binding);
      if (!target) {
        receipt = { sequence: offer.sequence, outcome: "refused", reason: "executionEnded" };
      } else {
        let authorized: WindowInputClaim;
        try {
          authorized = await this.port.authorize(offer.sequence);
        } catch (error) {
          // Runtime retires the exact pending claim before returning its typed refusal. Another receipt would
          // target a removed offer and close this healthy receiver. Transport or protocol loss stays unknown.
          if (error instanceof RuntimeRequestError) return;
          throw error;
        }
        if (this.closed || this.port.current(offer.binding) !== target) {
          receipt = { sequence: offer.sequence, outcome: "refused", reason: this.closed ? "receiverClosed" : "executionChanged" };
        } else if (Buffer.byteLength(authorized.text, "utf8") > PUBLIC_LIMITS.maxTerminalWriteBytes) {
          receipt = { sequence: offer.sequence, outcome: "refused", reason: "textTooLarge" };
        } else {
          try {
            target.sendText(authorized.text, false);
            receipt = { sequence: offer.sequence, outcome: "ownerExtensionAccepted" };
          } catch {
            // A throwing API may already have forwarded the call. Its unknown outcome cannot authorize retry.
            receipt = { sequence: offer.sequence, outcome: "outcomeUnknown", reason: "inputApiFailed" };
          }
        }
      }
      await this.port.complete(receipt);
    } finally {
      this.active = false;
    }
  }
}
