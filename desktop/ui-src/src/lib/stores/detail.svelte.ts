export interface PodRef {
  namespace: string;
  name: string;
}

/** The pod shown in the detail panel, if any. */
export const detail = $state({
  pod: null as PodRef | null,
  /** Bumped on every open so re-clicking the same pod reloads the panel. */
  nonce: 0,
});

export function openDetail(pod: PodRef) {
  detail.pod = { namespace: pod.namespace, name: pod.name };
  detail.nonce += 1;
}

export function closeDetail() {
  detail.pod = null;
}
