import { BinaryBitmap, DecodeHintType, HybridBinarizer, QRCodeReader, RGBLuminanceSource } from "@zxing/library";
import jsQR from "jsqr";

const reader = new QRCodeReader();
const hints = new Map([[DecodeHintType.TRY_HARDER, true]]);

// Reads a QR code from RGBA pixels. ZXing reads dense codes, such as pairing
// codes, that jsQR misses. jsQR also reads light codes on a dark background.
export function decodeQr(rgba: Uint8ClampedArray, width: number, height: number): string | null {
  const gray = new Uint8ClampedArray(width * height);
  for (let i = 0; i < gray.length; i++) {
    gray[i] = (rgba[i * 4] * 299 + rgba[i * 4 + 1] * 587 + rgba[i * 4 + 2] * 114) / 1000;
  }
  try {
    return reader.decode(new BinaryBitmap(new HybridBinarizer(new RGBLuminanceSource(gray, width, height))), hints).getText();
  } catch {
    // ZXing throws when the frame has no code. Try jsQR next.
  }
  return jsQR(rgba, width, height, { inversionAttempts: "attemptBoth" })?.data ?? null;
}
