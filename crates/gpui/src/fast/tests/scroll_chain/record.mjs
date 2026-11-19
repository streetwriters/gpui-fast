// Replays every scenario of fixture.json in Chrome and prints which boxes
// Chrome scrolled, against what the fixture expects.
//
//   cd "$(mktemp -d)" && npm i playwright && npx playwright install chromium
//   node /path/to/crates/gpui/src/fast/tests/scroll_chain/record.mjs
//
// Exits 1 when Chrome differs from the fixture anywhere. Pass --headed to
// watch it, --only=<name> for one scenario.
//
// Each scenario's boxes are nested divs with overflow auto on the axes they
// scroll and hidden on the others, no scrollbars and smooth scrolling off.
// A wheel tick is a CDP mouseWheel of 40 px a line (as Chrome on macOS sends
// one), shift+wheel sent as AppKit sends it, as deltaX. CDP wheel events
// carry no phase, so a trackpad gesture (its start to its end, with the
// momentum after it) is replayed as one synthesized scroll gesture of a
// mouse, which Chrome sends as phased wheel events: what a gesture latches
// is what this checks, not how each event of it moves.

import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(join(process.cwd(), 'package.json'));
const { chromium } = require('playwright');

const here = dirname(fileURLToPath(import.meta.url));
const fixture = JSON.parse(readFileSync(join(here, 'fixture.json'), 'utf8'));
const args = process.argv.slice(2);
const only = args.find((a) => a.startsWith('--only='))?.slice('--only='.length);
const LINE = 40;

function html(node) {
  const [w, h] = node.size;
  const [cw, ch] = node.content;
  const scrollsX = node.scroll.includes('x');
  const scrollsY = node.scroll.includes('y');
  const [cx, cy] = node.child_at ?? [0, 0];
  const child = node.child
    ? `<div style="position:absolute;left:${cx}px;top:${cy}px">${html(node.child)}</div>`
    : '';
  return (
    `<div id="${node.id}" style="width:${w}px;height:${h}px;` +
    `overflow-x:${scrollsX ? 'auto' : 'hidden'};overflow-y:${scrollsY ? 'auto' : 'hidden'};` +
    `scrollbar-width:none;flex:none">` +
    `<div style="position:relative;width:${cw}px;height:${ch}px">${child}</div></div>`
  );
}

function nodes(node) {
  return node.child ? [node, ...nodes(node.child)] : [node];
}

function words(before, after) {
  const out = [];
  if (after[1] > before[1]) out.push('down');
  if (after[1] < before[1]) out.push('up');
  if (after[0] > before[0]) out.push('right');
  if (after[0] < before[0]) out.push('left');
  return out.sort().join(' ');
}

const sorted = (w) => w.split(/\s+/).filter(Boolean).sort().join(' ');

/// The input as steps: wheel ticks, waits, pointer moves, and each trackpad
/// gesture with its momentum folded into one.
function steps(input) {
  const out = [];
  let gesture = null;
  for (const step of input) {
    if (step.trackpad) {
      if (step.trackpad === 'start') {
        gesture = { gesture: [0, 0] };
        out.push(gesture);
      } else if (step.trackpad === 'momentum' && !gesture) {
        throw new Error('momentum without a gesture');
      }
      gesture.gesture[0] += step.delta[0];
      gesture.gesture[1] += step.delta[1];
      continue;
    }
    gesture = null;
    out.push(step);
  }
  return out;
}

async function run(page, cdp, scenario) {
  await page.setContent(
    `<!doctype html><body style="margin:0">${html(scenario.layout)}</body>`,
  );
  const all = nodes(scenario.layout);
  await page.evaluate((all) => {
    for (const n of all) {
      const el = document.getElementById(n.id);
      el.scrollLeft = (n.start ?? [0, 0])[0];
      el.scrollTop = (n.start ?? [0, 0])[1];
    }
  }, all);
  const read = () =>
    page.evaluate(
      (ids) =>
        Object.fromEntries(
          ids.map((id) => {
            const el = document.getElementById(id);
            return [id, [el.scrollLeft, el.scrollTop]];
          }),
        ),
      all.map((n) => n.id),
    );
  const box = await page.evaluate((id) => {
    const r = document.getElementById(id).getBoundingClientRect();
    return [r.left + r.width / 2, r.top + r.height / 2];
  }, scenario.pointer);
  let [x, y] = box;
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
  const before = await read();
  for (const step of steps(scenario.input)) {
    if (step.wait !== undefined) {
      await page.waitForTimeout(step.wait);
    } else if (step.move_pointer) {
      x += step.move_pointer[0];
      y += step.move_pointer[1];
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
    } else if (step.wheel) {
      let [dx, dy] = step.wheel;
      if (step.shift && dx === 0) [dx, dy] = [dy, 0];
      await cdp.send('Input.dispatchMouseEvent', {
        type: 'mouseWheel',
        x,
        y,
        deltaX: dx * LINE,
        deltaY: dy * LINE,
        modifiers: step.shift ? 8 : 0,
      });
      await page.waitForTimeout(20);
    } else if (step.gesture) {
      // CDP's distances are the way the finger moves: positive scrolls up.
      await cdp.send('Input.synthesizeScrollGesture', {
        x,
        y,
        xDistance: -step.gesture[0],
        yDistance: -step.gesture[1],
        gestureSourceType: 'mouse',
        preventFling: true,
      });
      await page.waitForTimeout(20);
    }
  }
  await page.waitForTimeout(200);
  const after = await read();
  return Object.fromEntries(
    all.map((n) => [n.id, words(before[n.id], after[n.id])]),
  );
}

const browser = await chromium.launch({
  headless: !args.includes('--headed'),
  args: ['--disable-smooth-scrolling'],
});
const page = await browser.newPage({ viewport: { width: 400, height: 400 } });
const cdp = await page.context().newCDPSession(page);
let differs = 0;
for (const scenario of fixture.scenarios) {
  if (only && scenario.name !== only) continue;
  const moved = await run(page, cdp, scenario);
  const expected = Object.fromEntries(
    Object.keys(moved).map((id) => [id, sorted(scenario.expect[id] ?? '')]),
  );
  const same = JSON.stringify(moved) === JSON.stringify(expected);
  if (!same) differs += 1;
  console.log(
    `${same ? 'same   ' : 'DIFFERS'} ${scenario.name}: chrome ${JSON.stringify(moved)}` +
      (same ? '' : ` fixture ${JSON.stringify(expected)}`),
  );
}
await browser.close();
console.log(`${differs} of the scenarios differ from Chrome`);
process.exit(differs ? 1 : 0);
