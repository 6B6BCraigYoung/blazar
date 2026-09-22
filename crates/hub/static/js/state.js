const S = {
  nodes: [], workspaces: [], agents: [], profiles: [], runtimes: null, route: '',
  ws: null, editor: null, seen: new Set(),

  tree: [], treeRoot: null, opened: new Set(), changeOf: new Map(),

  open: [], file: null, models: new Map(), fileMeta: new Map(), dirtyShown: new Set(),
  term: null, termWs: null, fit: null, termResize: null,
  lay: null,

  session: null,

  navToken: 0,

  onStateChange: null,
};
