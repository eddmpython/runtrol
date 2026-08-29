import * as vscode from "vscode";

/// The part of the editor's built-in git extension this file reads: which repositories it has open and when
/// any of them changes. Typed here rather than imported, because the extension ships no declarations.
type GitRepository = {
  readonly rootUri: vscode.Uri;
  readonly state: { onDidChange: vscode.Event<void> };
};

type GitApi = {
  readonly repositories: readonly GitRepository[];
  readonly onDidOpenRepository: vscode.Event<GitRepository>;
  readonly onDidCloseRepository: vscode.Event<GitRepository>;
};

type GitExtensionExports = {
  getAPI(version: 1): GitApi;
};

/// Tell `changed` the root of every repository the editor's git extension sees change, for as long as this
/// window lives.
///
/// The git extension already watches the folders this window has open (a person committing by hand, a pull,
/// a stash) and says so through its API at no cost to us. Projects outside this window are not here; they are
/// measured when an agent in them writes. A window whose git extension is off simply gets no events from it.
export function followGitExtension(changed: (root: string) => void): vscode.Disposable {
  const subscriptions: vscode.Disposable[] = [];
  let disposed = false;
  const follow = (repository: GitRepository): void => {
    const listener = repository.state.onDidChange(() => changed(repository.rootUri.fsPath));
    subscriptions.push(listener);
  };
  void activateGit().then((api) => {
    if (disposed || !api) return;
    for (const repository of api.repositories) follow(repository);
    subscriptions.push(api.onDidOpenRepository(follow));
  });
  return {
    dispose: () => {
      disposed = true;
      for (const subscription of subscriptions) subscription.dispose();
    },
  };
}

async function activateGit(): Promise<GitApi | null> {
  const extension = vscode.extensions.getExtension<GitExtensionExports>("vscode.git");
  if (!extension) return null;
  const exports = extension.isActive ? extension.exports : await extension.activate();
  return exports.getAPI(1);
}
