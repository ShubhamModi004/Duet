// Browser end-to-end test of the guest viewer against a fake host.
// Run: cargo run -p duet-signal & (cd app/ui && python3 -m http.server 8899) &
//      node scripts/e2e-guest.js app/ui /tmp/guest.png   (needs `npm i playwright`)
const { chromium } = require('playwright');
const UI = process.argv[2];
(async () => {
  const browser = await chromium.launch({ args: ['--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream'] });
  const ctx = await browser.newContext();
  // Fake host: a canvas stream + control channel, speaking the Duet protocol.
  const hostPage = await ctx.newPage();
  await hostPage.goto('http://127.0.0.1:8899/index.html');
  await hostPage.evaluate(() => {
    window.got = [];
    const ws = new WebSocket('ws://127.0.0.1:8787/ws');
    const c = document.createElement('canvas'); c.width = 640; c.height = 360;
    const g = c.getContext('2d'); setInterval(() => { g.fillStyle = '#' + Math.floor(Math.random()*0xffffff).toString(16).padStart(6,'0'); g.fillRect(0,0,640,360); }, 50);
    const stream = c.captureStream(30);
    const pcs = {};
    ws.onopen = () => ws.send(JSON.stringify({ type: 'join', room: 'e2e-room', name: 'Host', role: 'host' }));
    ws.onmessage = async (e) => {
      const m = JSON.parse(e.data);
      if (m.type === 'peer_joined') {
        const id = m.peer.id;
        const pc = new RTCPeerConnection(); pcs[id] = pc;
        stream.getTracks().forEach(t => pc.addTrack(t, stream));
        const dc = pc.createDataChannel('control'); window.dc = dc;
        dc.onopen = () => dc.send(JSON.stringify({ type: 'hello', name: 'Host', os: 'windows' }));
        dc.onmessage = (ev) => { const msg = JSON.parse(ev.data); window.got.push(msg); if (msg.type === 'request_control') dc.send(JSON.stringify({ type: 'grant_control', to: id })); };
        pc.ontrack = (ev) => { window.gotAudio = ev.track.kind; };
        pc.onicecandidate = (ev) => ev.candidate && ws.send(JSON.stringify({ type: 'signal', to: id, payload: { ice: ev.candidate.toJSON() } }));
        const offer = await pc.createOffer(); await pc.setLocalDescription(offer);
        ws.send(JSON.stringify({ type: 'signal', to: id, payload: { sdp: { type: 'offer', sdp: offer.sdp } } }));
      } else if (m.type === 'signal') {
        const pc = pcs[m.from];
        if (m.payload.sdp) await pc.setRemoteDescription(m.payload.sdp);
        else if (m.payload.ice) await pc.addIceCandidate(m.payload.ice);
      }
    };
  });
  await hostPage.waitForTimeout(500);

  const guest = await ctx.newPage();
  guest.on('console', (m) => console.log('[guest]', m.text()));
  guest.on('pageerror', (e) => console.log('[guest error]', e.message));
  await guest.addInitScript(() => { window.__TAURI__ = { core: { invoke: async () => {} }, event: { listen: async () => () => {} } }; });
  await guest.setViewportSize({ width: 1000, height: 700 });
  await guest.goto('http://127.0.0.1:8899/viewer.html?room=e2e-room&name=Lin&server=' + encodeURIComponent('ws://127.0.0.1:8787/ws'));
  await guest.waitForFunction(() => document.getElementById('screen').videoWidth > 0, null, { timeout: 15000 });
  console.log('video size', await guest.evaluate(() => [screen.videoWidth, document.getElementById('screen').videoWidth, document.getElementById('screen').videoHeight]));
  console.log('status', await guest.textContent('#guest-status'));
  // Point mode: move over the stage.
  const box = await guest.locator('#stage').boundingBox();
  await guest.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await guest.waitForTimeout(300);
  // Control mode
  await guest.click('#mode-control');
  await guest.waitForTimeout(500);
  console.log('status after control', await guest.textContent('#guest-status'));
  await guest.mouse.move(box.x + box.width / 2 + 10, box.y + box.height / 2);
  await guest.mouse.down(); await guest.mouse.up();
  await guest.keyboard.type('hi');
  await guest.keyboard.press('Control+KeyC');
  await guest.mouse.wheel(0, 120);
  await guest.waitForTimeout(500);
  const got = await hostPage.evaluate(() => window.got);
  console.log('host received:', JSON.stringify(got.map(m => m.type)));
  console.log('sample:', JSON.stringify(got.filter(m => ['pointer','mouse_down','key_down','text','hello'].includes(m.type)).slice(0,6)));
  console.log('host got guest audio track:', await hostPage.evaluate(() => window.gotAudio));
  await guest.screenshot({ path: process.argv[3] });
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
