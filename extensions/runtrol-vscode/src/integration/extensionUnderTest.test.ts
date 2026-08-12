import * as vscode from "vscode";

export function extensionUnderTest<Api>(): vscode.Extension<Api> {
  const identifier = process.env.RUNTROL_TEST_EXTENSION_ID;
  if (!identifier) {
    throw new Error("RUNTROL_TEST_EXTENSION_ID is required");
  }
  const extension = vscode.extensions.getExtension<Api>(identifier);
  if (!extension) {
    throw new Error(`the Runtrol Studio extension ${identifier} is missing`);
  }
  return extension;
}
