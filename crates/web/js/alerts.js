// 提醒：提示音（WebAudio 合成，不带音频文件）和系统通知。
const TONES = { soft: ['sine', 1], bright: ['triangle', 1.5], wood: ['square', 0.5] };
let ctx;

export function playSound(kind, tone, volume) {
  try {
    ctx = ctx || new (window.AudioContext || window.webkitAudioContext)();
    if (ctx.state === 'suspended') ctx.resume();
    const [wave, mult] = TONES[tone] || TONES.soft;
    const notes = kind === 'attention' ? [[880, 0, .09], [880, .13, .09], [1175, .26, .16]] : [[659, 0, .14], [988, .15, .26]];
    const t0 = ctx.currentTime + 0.02;
    for (const [f, at, dur] of notes) {
      const o = ctx.createOscillator(), g = ctx.createGain();
      o.type = wave; o.frequency.value = f * mult;
      const peak = Math.max(0.0001, volume * (wave === 'square' ? 0.12 : 0.3));
      g.gain.setValueAtTime(0.0001, t0 + at);
      g.gain.exponentialRampToValueAtTime(peak, t0 + at + 0.012);
      g.gain.exponentialRampToValueAtTime(0.0001, t0 + at + dur);
      o.connect(g).connect(ctx.destination);
      o.start(t0 + at); o.stop(t0 + at + dur + 0.02);
    }
  } catch (_) { /* 没有声音就算了 */ }
}

export function notifySupported() { return typeof Notification !== 'undefined'; }
export function notifyGranted() { return notifySupported() && Notification.permission === 'granted'; }
export async function requestNotify() {
  if (!notifySupported()) return 'unsupported';
  return Notification.permission === 'granted' ? 'granted' : await Notification.requestPermission();
}

// 弹一条系统通知；点一下回到窗口并调 onClick。
export function notify(title, body, tag, sticky, onClick) {
  try {
    const n = new Notification(title, { body, tag, requireInteraction: sticky });
    n.onclick = () => { window.focus(); onClick(); n.close(); };
  } catch (_) {}
}
