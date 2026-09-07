import * as QRCode from "qrcode/lib/core/qrcode";
import * as SvgRenderer from "qrcode/lib/renderer/svg-tag";

export { pairPhone, managePhones, reviewPhonePairings } from "./pairingAdministration";

export function render(value: string): string {
  return SvgRenderer.render(QRCode.create(value));
}
