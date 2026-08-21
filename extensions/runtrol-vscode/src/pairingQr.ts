import * as QRCode from "qrcode/lib/core/qrcode";
import * as SvgRenderer from "qrcode/lib/renderer/svg-tag";

export function pairingQrDataUrl(value: string): string {
  const svg = SvgRenderer.render(QRCode.create(value));
  return `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
}
