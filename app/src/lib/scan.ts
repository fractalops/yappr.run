import { decodeQr } from "./qr";

/** The longest side of a frame that the decoder reads, in pixels. A pairing
 * code is a dense QR code, so the decoder needs a large frame. */
const FRAME = 1080;
/** How long the success state stays on screen before the scanner closes. */
const SUCCESS_MS = 650;

const SCANNER_STYLE = `
.yappr-scan { position: fixed; inset: 0; z-index: 1000; background: #000; display: flex;
  flex-direction: column; align-items: center; justify-content: center; gap: 18px; padding: 16px; }
.yappr-frame { position: relative; width: min(78vw, 360px); aspect-ratio: 1; border-radius: 18px;
  overflow: hidden; box-shadow: inset 0 0 0 2px #ffffff40; transition: box-shadow .25s ease; }
.yappr-frame video { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: cover; }
.yappr-line { position: absolute; left: 6%; right: 6%; height: 2px; top: 8%;
  background: linear-gradient(90deg, transparent, #7fe0a6, transparent); box-shadow: 0 0 12px #7fe0a6;
  animation: yappr-sweep 1.8s ease-in-out infinite alternate; }
.yappr-tick { position: absolute; inset: 0; display: none; align-items: center; justify-content: center;
  background: #0b2a1866; }
.yappr-tick span { width: 84px; height: 84px; box-shadow: 0 6px 24px #0008; border-radius: 50%; background: #3ccf7a; color: #06210f;
  font: 600 46px -apple-system, sans-serif; display: flex; align-items: center; justify-content: center;
  animation: yappr-pop .3s ease-out; }
.yappr-frame.found { box-shadow: inset 0 0 0 5px #3ccf7a, 0 0 36px #3ccf7aaa; }
.yappr-frame.found .yappr-line { display: none; }
.yappr-frame.found .yappr-tick { display: flex; }
.yappr-scan p { color: #e0e3e8; font: 14px -apple-system, sans-serif; margin: 0; }
.yappr-scan button { min-height: 44px; padding: 0 24px; border-radius: 8px; border: 1px solid #484f65;
  background: #20232a; color: #e0e3e8; font: 15px -apple-system, sans-serif; }
.yappr-scan button:disabled { opacity: .4; }
@keyframes yappr-sweep { from { top: 8%; opacity: .35; } 50% { opacity: 1; } to { top: 92%; opacity: .35; } }
@keyframes yappr-pop { from { transform: scale(.4); opacity: 0; } to { transform: scale(1); opacity: 1; } }
@media (prefers-reduced-motion: reduce) {
  .yappr-line { animation: none; top: 50%; }
  .yappr-tick span { animation: none; }
}
`;

// Opens the rear camera in a full-screen view and returns the first QR code
// that it reads. Cancel rejects with "cancelled".
export async function scan(): Promise<{ content: string }> {
  if (!navigator.mediaDevices?.getUserMedia) {
    throw "This browser has no camera access.";
  }
  let stream: MediaStream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({
      video: { facingMode: "environment", width: { ideal: 1920 }, height: { ideal: 1080 } },
      audio: false,
    });
  } catch {
    throw "Allow camera access for this page.";
  }

  const style = document.createElement("style");
  style.textContent = SCANNER_STYLE;
  const overlay = document.createElement("div");
  overlay.className = "yappr-scan";
  const frame = document.createElement("div");
  frame.className = "yappr-frame";
  const video = document.createElement("video");
  video.setAttribute("playsinline", "");
  video.muted = true;
  video.srcObject = stream;
  const line = document.createElement("div");
  line.className = "yappr-line";
  const tick = document.createElement("div");
  tick.className = "yappr-tick";
  tick.innerHTML = "<span>\u2713</span>";
  frame.append(video, line, tick);
  const hint = document.createElement("p");
  hint.textContent = "Point the camera at a Yappr QR code.";
  const cancel = document.createElement("button");
  cancel.textContent = "Cancel";
  overlay.append(style, frame, hint, cancel);
  document.body.append(overlay);

  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d", { willReadFrequently: true });
  return new Promise((resolve, reject) => {
    let done = false;
    const stop = () => {
      done = true;
      stream.getTracks().forEach((track) => track.stop());
      overlay.remove();
    };
    cancel.onclick = () => {
      stop();
      reject("cancelled");
    };
    const read = () => {
      if (done) return;
      if (context && video.readyState >= video.HAVE_CURRENT_DATA && video.videoWidth) {
        const scale = Math.min(1, FRAME / Math.max(video.videoWidth, video.videoHeight));
        const width = Math.round(video.videoWidth * scale);
        const height = Math.round(video.videoHeight * scale);
        canvas.width = width;
        canvas.height = height;
        context.drawImage(video, 0, 0, width, height);
        const text = decodeQr(context.getImageData(0, 0, width, height).data, width, height);
        if (text) {
          // Show the success state on the last frame, then close.
          done = true;
          video.pause();
          frame.classList.add("found");
          hint.textContent = "Code read";
          cancel.disabled = true;
          navigator.vibrate?.(30);
          setTimeout(() => {
            stop();
            resolve({ content: text });
          }, SUCCESS_MS);
          return;
        }
      }
      requestAnimationFrame(read);
    };
    video.play().then(() => requestAnimationFrame(read), (error) => {
      stop();
      reject(String(error));
    });
  });
}
