declare module "qrcode/lib/core/qrcode" {
  import type { QRCodeToStringOptions } from "qrcode";

  export function create(text: string, options?: QRCodeToStringOptions): unknown;
}

declare module "qrcode/lib/renderer/svg-tag" {
  import type { QRCodeToStringOptions } from "qrcode";

  export function render(data: unknown, options?: QRCodeToStringOptions): string;
}
