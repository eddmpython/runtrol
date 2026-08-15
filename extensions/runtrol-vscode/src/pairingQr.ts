import * as QRCode from "qrcode/lib/browser";

export async function pairingQrDataUrl(value: string): Promise<string> {
  const svg = await QRCode.toString(value, {
    type: "svg",
    errorCorrectionLevel: "M",
    margin: 3,
    width: 320,
    color: { dark: "#101418", light: "#ffffff" },
  });
  return `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
}
