type PairingQrVendor = {
  render(value: string): string;
};

let vendor: PairingQrVendor | null = null;

export function pairingQrDataUrl(value: string): string {
  // The encoder is needed only while the pairing panel is open. Keeping it in a bounded sibling bundle saves
  // every ordinary Extension Host activation from loading the QR tables and segmentation implementation.
  vendor ??= require("./pairingQrVendor") as PairingQrVendor;
  const svg = vendor.render(value);
  return `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;
}
