// The wasm route's loader.
//
// Nothing here stands in for dlopen. A module is fetched and instantiated the
// way wasm modules are instantiated, and its exports are handed to the engine.
// Swapping a module is instantiating another one and handing over those
// exports instead.
import init, { describeModule, engineImports, openGame } from './pkg/se_web.js';

const out = document.getElementById('out');
const say = (s, bad) => {
  const line = document.createElement('div');
  if (bad) line.className = 'bad';
  line.textContent = s;
  out.appendChild(line);
};

// A control module imports the host by name — `se_ctl_spawn`, `se_ctl_set`,
// … — because a wasm module cannot be *handed* a host function: a pointer
// means nothing across the boundary. `engineImports()` is what answers them,
// and it closes over the engine's own world.
async function loadModule(url) {
  const bytes = await (await fetch(url)).arrayBuffer();
  const instance = await WebAssembly.instantiate(await WebAssembly.compile(bytes), engineImports());
  return instance.exports;
}

// Which modules go into the world.
//
// Natively this is `game.so`'s job: it owns which module fills which slot. The
// tick below now says what it *wants* bound, but nothing acts on that yet, so
// the page still chooses — one render module, one asset module and one game
// module, the same shape of answer, arrived at from the other end. `?graph=`,
// `?asset=` and `?control=` override.
function slot(modules, kind, want) {
  const of = modules.filter(m => m.startsWith(kind + '_'));
  if (!of.length) return [];
  if (!want) return [of[0]];
  const hit = of.find(m => m === `${kind}_${want}`);
  if (!hit) throw new Error(`no ${kind} module named \`${want}\` (have ${of.join(', ')})`);
  return [hit];
}

async function main() {
  out.textContent = '';
  await init();
  say('engine started');

  const q = new URLSearchParams(location.search);
  const game = q.get('game') || 'game1';
  const modules = (await (await fetch(`./games/${game}/modules.json`)).json());
  say(`${game}: ${modules.length} modules`);

  const exports = {};
  for (const name of modules) {
    try {
      exports[name] = await loadModule(`./games/${game}/${name}.wasm`);
      const text = describeModule(name, exports[name]);
      say('');
      say(`── ${name} ──`);
      for (const l of text.split('\n')) if (l) say('  ' + l);
    } catch (e) {
      say(`${name}: ${e}`, true);
    }
  }

  // One world, one slot: the bundle for layouts and buffers, every stage, one
  // graph, one theme, and the game module whose tick writes what the graph
  // draws. They go in together because they are one game — a tick and a frame
  // standing up their own stores were about two different worlds, which is the
  // bug this handle exists to close.
  //
  // `process` is not a `slot()` call: natively a category nobody bound loads
  // *all* of it, which is what makes adding a stage additive. A bound Process
  // slot would narrow this, and nothing binds one yet.
  const chosen = [
    ...modules.filter(m => m === 'bundle'),
    ...modules.filter(m => m.startsWith('process_')),
    ...slot(modules, 'render', q.get('graph')),
    ...slot(modules, 'asset', q.get('asset')),
    ...slot(modules, 'game', q.get('control')),
  ];
  const canvas = document.getElementById('se');
  if (q.get('w')) canvas.width = Number(q.get('w'));
  if (q.get('h')) canvas.height = Number(q.get('h'));

  say('');
  say(`── world ── ${chosen.join(' + ')}`);
  // Named `world` because that is what it holds: `game` above is the folder.
  const world = openGame(chosen, chosen.map(n => exports[n]), canvas.width, canvas.height);

  // Control first, then the graph — design.md's order, and the whole point:
  // the frame below shows what these ticks spawned and moved.
  const ticks = Number(q.get('ticks') || 1);
  try {
    for (const l of world.tick(ticks).split('\n')) if (l) say('  ' + l);
  } catch (e) {
    say(`control: ${e}`, true);
  }

  say('');
  say('── frame ──');
  say('  ' + await world.render(canvas));
}

main().catch(e => say(String(e), true));
