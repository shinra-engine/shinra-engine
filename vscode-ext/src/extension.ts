import * as http from 'http';
import * as vscode from 'vscode';

const HTTP_PORT = 5812;
const WS_PORT = 5813;

// ---- scene types mirroring scene/src/lib.rs --------------------------------

interface Transform {
    translation: [number, number, number];
    rotation: [number, number, number, number];
    scale: [number, number, number];
}

interface SceneNodeData {
    name: string;
    transform?: Transform;
    mesh?: { path: string };
    tilemap?: unknown;
    components?: string[];
    children?: SceneNodeData[];
}

interface SceneData {
    name: string;
    nodes: SceneNodeData[];
}

// ---- scene tree view -------------------------------------------------------

class SceneNode extends vscode.TreeItem {
    constructor(
        public readonly nodeData: SceneNodeData,
        collapsibleState: vscode.TreeItemCollapsibleState,
        public readonly nodePath: number[],
    ) {
        super(nodeData.name, collapsibleState);
        this.contextValue = 'sceneNode';
        if (nodeData.mesh) {
            this.description = nodeData.mesh.path.replace(/^.*[\\/]/, '');
            this.iconPath = new vscode.ThemeIcon('symbol-object');
        } else if (nodeData.components && nodeData.components.length > 0) {
            this.description = nodeData.components.join(', ');
            this.iconPath = new vscode.ThemeIcon('symbol-event');
        } else {
            this.iconPath = new vscode.ThemeIcon('symbol-namespace');
        }
    }
}

class SceneTreeProvider implements vscode.TreeDataProvider<SceneNode>, vscode.Disposable {
    private readonly _onDidChangeTreeData = new vscode.EventEmitter<void>();
    readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

    private sceneJson = '';
    private sceneData: SceneData | null = null;
    private readonly timer: NodeJS.Timeout;

    constructor() {
        this.timer = setInterval(() => this.fetchScene(), 3000);
        this.fetchScene();
    }

    dispose(): void {
        clearInterval(this.timer);
        this._onDidChangeTreeData.dispose();
    }

    refresh(): void {
        this.fetchScene();
    }

    getTreeItem(element: SceneNode): vscode.TreeItem {
        return element;
    }

    getChildren(element?: SceneNode): SceneNode[] {
        if (!this.sceneData) {
            return [];
        }
        const basePath = element ? element.nodePath : [];
        const nodes = element ? (element.nodeData.children ?? []) : this.sceneData.nodes;
        return nodes.map((n, i) => {
            const state =
                n.children && n.children.length > 0
                    ? vscode.TreeItemCollapsibleState.Collapsed
                    : vscode.TreeItemCollapsibleState.None;
            return new SceneNode(n, state, [...basePath, i]);
        });
    }

    private fetchScene(): void {
        const req = http.get(`http://localhost:${HTTP_PORT}/scene`, (res) => {
            let raw = '';
            res.on('data', (chunk: Buffer) => { raw += chunk.toString(); });
            res.on('end', () => {
                try {
                    if (raw !== this.sceneJson) {
                        this.sceneJson = raw;
                        this.sceneData = JSON.parse(raw) as SceneData;
                        this._onDidChangeTreeData.fire();
                    }
                } catch {
                    // server returned non-JSON; ignore
                }
            });
        });
        req.on('error', () => { /* server not running */ });
        req.end();
    }
}

// ---- inspector WebviewView -------------------------------------------------

class InspectorViewProvider implements vscode.WebviewViewProvider, vscode.Disposable {
    static readonly viewType = 'shinraInspector';
    private view?: vscode.WebviewView;

    resolveWebviewView(webviewView: vscode.WebviewView): void {
        this.view = webviewView;
        webviewView.webview.options = { enableScripts: true };
        webviewView.webview.html = buildInspectorHtml();
        webviewView.webview.onDidReceiveMessage((msg: unknown) => {
            const m = msg as { type: string; nodePath: number[]; node: SceneNodeData };
            if (m.type === 'apply') {
                applyNodePatch(m.nodePath, m.node);
            }
        });
    }

    showNode(node: SceneNodeData | null, nodePath: number[]): void {
        if (this.view) {
            this.view.webview.postMessage({ type: 'show', node, nodePath });
        }
    }

    dispose(): void {}
}

function resolveParentList(roots: SceneNodeData[], nodePath: number[]): SceneNodeData[] | null {
    if (nodePath.length === 0) { return null; }
    let list = roots;
    for (let i = 0; i < nodePath.length - 1; i++) {
        const next = list[nodePath[i]];
        if (!next?.children) { return null; }
        list = next.children;
    }
    return list;
}

function applyNodePatch(nodePath: number[], updated: SceneNodeData): void {
    const req = http.get(`http://localhost:${HTTP_PORT}/scene`, (res) => {
        let raw = '';
        res.on('data', (chunk: Buffer) => { raw += chunk.toString(); });
        res.on('end', () => {
            try {
                const scene = JSON.parse(raw) as SceneData;
                const parentList = resolveParentList(scene.nodes, nodePath);
                if (!parentList) { return; }
                const idx = nodePath[nodePath.length - 1];
                const existing = parentList[idx];
                if (!existing) { return; }
                parentList[idx] = {
                    ...existing,
                    name: updated.name,
                    ...(updated.transform ? { transform: updated.transform } : {}),
                    ...(existing.mesh && updated.mesh ? { mesh: updated.mesh } : {}),
                };
                postScene(scene);
            } catch {
                // parse or network error
            }
        });
    });
    req.on('error', () => {});
    req.end();
}

function postScene(scene: SceneData): void {
    const body = JSON.stringify(scene);
    const opts: http.RequestOptions = {
        hostname: 'localhost',
        port: HTTP_PORT,
        path: '/scene',
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(body),
        },
    };
    const req = http.request(opts, () => {});
    req.on('error', () => {});
    req.write(body);
    req.end();
}

function httpPost(path: string, body: object): void {
    const bodyStr = JSON.stringify(body);
    const opts: http.RequestOptions = {
        hostname: 'localhost',
        port: HTTP_PORT,
        path,
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
            'Content-Length': Buffer.byteLength(bodyStr),
        },
    };
    const req = http.request(opts, () => {});
    req.on('error', () => {});
    req.write(bodyStr);
    req.end();
}

// ---- extension entry points ------------------------------------------------

export function activate(ctx: vscode.ExtensionContext): void {
    const sceneProvider = new SceneTreeProvider();
    const inspectorProvider = new InspectorViewProvider();

    const treeView = vscode.window.createTreeView('shinraSceneTree', {
        treeDataProvider: sceneProvider,
    });

    ctx.subscriptions.push(
        sceneProvider,
        treeView,
        inspectorProvider,
        treeView.onDidChangeSelection((e) => {
            const sel = e.selection[0];
            inspectorProvider.showNode(sel ? sel.nodeData : null, sel ? sel.nodePath : []);
        }),
        vscode.window.registerWebviewViewProvider(InspectorViewProvider.viewType, inspectorProvider),
        vscode.commands.registerCommand('shinra.refreshScene', () => sceneProvider.refresh()),
        vscode.commands.registerCommand('shinra.openViewport', () => {
            ViewportPanel.createOrShow();
        }),
        vscode.commands.registerCommand('shinra.saveScene', async () => {
            const path = await vscode.window.showInputBox({
                prompt: 'Save scene to path (.scn.ron)',
                value: 'assets/scenes/scene.scn.ron',
            });
            if (!path) { return; }
            httpPost('/scene/save', { path });
        }),
        vscode.commands.registerCommand('shinra.loadScene', async () => {
            const path = await vscode.window.showInputBox({
                prompt: 'Load scene from path (.scn.ron)',
                value: 'assets/scenes/scene.scn.ron',
            });
            if (!path) { return; }
            httpPost('/scene/load', { path });
            sceneProvider.refresh();
        }),
    );
}

export function deactivate(): void {}

// ---- viewport panel --------------------------------------------------------

class ViewportPanel {
    static readonly viewType = 'shinraViewport';
    private static instance: ViewportPanel | undefined;

    private readonly panel: vscode.WebviewPanel;

    private constructor() {
        this.panel = vscode.window.createWebviewPanel(
            ViewportPanel.viewType,
            'Shinra Viewport',
            vscode.ViewColumn.One,
            {
                enableScripts: true,
                retainContextWhenHidden: true,
            }
        );
        this.panel.webview.html = buildViewportHtml();
        this.panel.onDidDispose(() => {
            ViewportPanel.instance = undefined;
        });
    }

    static createOrShow(): void {
        if (ViewportPanel.instance) {
            ViewportPanel.instance.panel.reveal(vscode.ViewColumn.One);
        } else {
            ViewportPanel.instance = new ViewportPanel();
        }
    }
}

function buildViewportHtml(): string {
    return `<!doctype html>
<html>
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="
    default-src 'none';
    connect-src ws://localhost:${WS_PORT};
    style-src 'unsafe-inline';
    script-src 'unsafe-inline';
  ">
  <style>
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body { background: #111; display: flex; flex-direction: column; height: 100vh; font-family: monospace; }
    #viewport { flex: 1; display: flex; align-items: center; justify-content: center; overflow: hidden; }
    #canvas { max-width: 100%; max-height: 100%; object-fit: contain; display: block; }
    #statusbar { color: #666; font-size: 11px; padding: 3px 8px; border-top: 1px solid #222; }
  </style>
</head>
<body>
  <div id="viewport">
    <canvas id="canvas" width="512" height="384"></canvas>
  </div>
  <div id="statusbar">connecting…</div>
  <script>
    const statusbar = document.getElementById('statusbar');
    const canvas = document.getElementById('canvas');
    const ctx = canvas.getContext('2d');

    const decoder = new VideoDecoder({
      output(frame) { ctx.drawImage(frame, 0, 0); frame.close(); },
      error(e) { console.warn('decoder:', e); },
    });
    decoder.configure({ codec: 'avc1.42E01E', codedWidth: 512, codedHeight: 384 });

    let synced = false;
    let ws;
    function connectWs() {
      ws = new WebSocket('ws://localhost:${WS_PORT}/ws');
      ws.binaryType = 'arraybuffer';
      ws.onopen = () => { statusbar.textContent = 'connected  |  run: cargo run -p editor-server'; };
      ws.onclose = () => {
        synced = false;
        statusbar.textContent = 'ws disconnected — retrying in 2s…';
        setTimeout(connectWs, 2000);
      };
      ws.onmessage = ({ data }) => {
        const buf = new Uint8Array(data);
        const isKey = buf[0] === 1;
        if (!synced && !isKey) return;
        synced = true;
        if (decoder.state !== 'closed') {
          decoder.decode(new EncodedVideoChunk({
            type: isKey ? 'key' : 'delta',
            timestamp: performance.now() * 1000,
            data: buf.subarray(1),
          }));
        }
      };
    }
    connectWs();

    document.addEventListener('keydown', e => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'keydown', key: e.key }));
      }
    });
    document.addEventListener('keyup', e => {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: 'keyup', key: e.key }));
      }
    });
  </script>
</body>
</html>`;
}

function buildInspectorHtml(): string {
    return `<!doctype html>
<html>
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';">
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      font-family: var(--vscode-font-family, sans-serif);
      font-size: var(--vscode-font-size, 13px);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 8px;
    }
    h2 {
      font-size: 10px;
      text-transform: uppercase;
      letter-spacing: 0.08em;
      color: var(--vscode-descriptionForeground);
      margin: 10px 0 4px;
    }
    .row { display: flex; gap: 4px; margin-bottom: 3px; align-items: center; }
    .lbl { width: 16px; font-size: 11px; color: var(--vscode-descriptionForeground); flex-shrink: 0; text-align: right; }
    input[type=text], input[type=number] {
      flex: 1;
      background: var(--vscode-input-background);
      border: 1px solid var(--vscode-input-border, #555);
      color: var(--vscode-input-foreground);
      padding: 2px 5px;
      border-radius: 2px;
      font-size: inherit;
      font-family: monospace;
    }
    input:focus { outline: 1px solid var(--vscode-focusBorder); border-color: transparent; }
    button {
      margin-top: 10px;
      width: 100%;
      padding: 5px;
      background: var(--vscode-button-background);
      color: var(--vscode-button-foreground);
      border: none;
      border-radius: 2px;
      cursor: pointer;
      font-size: inherit;
      font-family: inherit;
    }
    button:hover { background: var(--vscode-button-hoverBackground); }
    #empty { color: var(--vscode-descriptionForeground); font-style: italic; font-size: 12px; }
  </style>
</head>
<body>
  <p id="empty">Select a node in the scene tree.</p>
  <div id="form" style="display:none">
    <h2>Node</h2>
    <div class="row">
      <span class="lbl"></span>
      <input type="text" id="name" placeholder="name">
    </div>

    <h2>Translation</h2>
    <div class="row"><span class="lbl">X</span><input type="number" id="tx" step="0.1"></div>
    <div class="row"><span class="lbl">Y</span><input type="number" id="ty" step="0.1"></div>
    <div class="row"><span class="lbl">Z</span><input type="number" id="tz" step="0.1"></div>

    <h2>Rotation (quat)</h2>
    <div class="row"><span class="lbl">X</span><input type="number" id="rx" step="0.01"></div>
    <div class="row"><span class="lbl">Y</span><input type="number" id="ry" step="0.01"></div>
    <div class="row"><span class="lbl">Z</span><input type="number" id="rz" step="0.01"></div>
    <div class="row"><span class="lbl">W</span><input type="number" id="rw" step="0.01"></div>

    <h2>Scale</h2>
    <div class="row"><span class="lbl">X</span><input type="number" id="sx" step="0.1"></div>
    <div class="row"><span class="lbl">Y</span><input type="number" id="sy" step="0.1"></div>
    <div class="row"><span class="lbl">Z</span><input type="number" id="sz" step="0.1"></div>

    <div id="mesh-section">
      <h2>Mesh</h2>
      <div class="row">
        <span class="lbl"></span>
        <input type="text" id="mesh-path" placeholder="assets/…obj">
      </div>
    </div>

    <button id="apply-btn">Apply</button>
  </div>
  <script>
    const vscode = acquireVsCodeApi();
    const form = document.getElementById('form');
    const empty = document.getElementById('empty');
    const meshSection = document.getElementById('mesh-section');
    let currentNodePath = [];
    let hasMesh = false;

    function num(id) { return parseFloat(document.getElementById(id).value) || 0; }
    function val(id) { return document.getElementById(id).value; }
    function set(id, v) { document.getElementById(id).value = v; }

    window.addEventListener('message', ({ data }) => {
      if (data.type !== 'show') { return; }
      if (!data.node) {
        form.style.display = 'none';
        empty.style.display = '';
        return;
      }
      form.style.display = '';
      empty.style.display = 'none';
      currentNodePath = data.nodePath;
      const n = data.node;
      const t = n.transform?.translation ?? [0, 0, 0];
      const r = n.transform?.rotation ?? [0, 0, 0, 1];
      const s = n.transform?.scale ?? [1, 1, 1];
      set('name', n.name ?? '');
      set('tx', t[0]); set('ty', t[1]); set('tz', t[2]);
      set('rx', r[0]); set('ry', r[1]); set('rz', r[2]); set('rw', r[3]);
      set('sx', s[0]); set('sy', s[1]); set('sz', s[2]);
      hasMesh = !!n.mesh;
      meshSection.style.display = hasMesh ? '' : 'none';
      if (hasMesh) { set('mesh-path', n.mesh.path ?? ''); }
    });

    document.getElementById('apply-btn').addEventListener('click', () => {
      const node = {
        name: val('name'),
        transform: {
          translation: [num('tx'), num('ty'), num('tz')],
          rotation: [num('rx'), num('ry'), num('rz'), num('rw')],
          scale: [num('sx'), num('sy'), num('sz')],
        },
      };
      if (hasMesh) { node.mesh = { path: val('mesh-path') }; }
      vscode.postMessage({ type: 'apply', nodePath: currentNodePath, node });
    });
  </script>
</body>
</html>`;
}
