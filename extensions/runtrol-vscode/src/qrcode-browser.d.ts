declare module "qrcode/lib/browser" {
  import type { QRCodeToStringOptions } from "qrcode";

  export function toString(text: string, options?: QRCodeToStringOptions): Promise<string>;
}
