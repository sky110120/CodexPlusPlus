export type ImplicitRelayModelCandidate = {
  storedModel: string;
  storedConfigModel: string;
  liveModel: string;
  modelListHead: string;
  modelFieldsUntouched: boolean;
};

export type RelayModelDraft = {
  model: string;
  configContents: string;
};

export function isImplicitRelayModelFallback(candidate: ImplicitRelayModelCandidate): boolean {
  const head = candidate.modelListHead.trim();
  return candidate.modelFieldsUntouched
    && !candidate.storedModel.trim()
    && !candidate.storedConfigModel.trim()
    && Boolean(head)
    && candidate.liveModel.trim() === head;
}

export function relayModelFieldsChanged(
  previous: RelayModelDraft,
  next: RelayModelDraft,
  readConfigModel: (contents: string) => string,
): boolean {
  return previous.model !== next.model
    || readConfigModel(previous.configContents) !== readConfigModel(next.configContents);
}

export function reconcileRelayModelDraft<T extends RelayModelDraft>(
  stored: RelayModelDraft,
  currentDraft: T,
  liveDraft: T,
  candidate: Pick<ImplicitRelayModelCandidate, "liveModel" | "modelListHead">,
  modelFieldsTouched: boolean,
  readConfigModel: (contents: string) => string,
  setConfigModel: (contents: string, model: string) => string,
): T {
  if (modelFieldsTouched) {
    return {
      ...liveDraft,
      model: currentDraft.model,
      configContents: setConfigModel(
        liveDraft.configContents,
        readConfigModel(currentDraft.configContents),
      ),
    };
  }

  if (isImplicitRelayModelFallback({
    ...candidate,
    modelFieldsUntouched: true,
    storedModel: stored.model,
    storedConfigModel: readConfigModel(stored.configContents),
  })) {
    return {
      ...liveDraft,
      model: "",
      configContents: setConfigModel(liveDraft.configContents, ""),
    };
  }

  return liveDraft;
}
