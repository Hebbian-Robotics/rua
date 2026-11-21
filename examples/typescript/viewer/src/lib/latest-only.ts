// INVARIANT: Allow one in-flight request and retain only the newest pending value. A failed
// send must not block the next pending request.
export function createLatestOnly<Value>(
  send: (value: Value) => Promise<void>,
  onError: (error: unknown) => void = () => {},
) {
  let waiting: { value: Value } | null = null;
  let draining: Promise<void> | null = null;

  async function drain(): Promise<void> {
    while (waiting) {
      const { value } = waiting;
      waiting = null;
      try {
        await send(value);
      } catch (error: unknown) {
        onError(error);
      }
    }
    draining = null;
  }

  return {
    push(value: Value): void {
      waiting = { value };
      draining ??= drain();
    },
    // CONSTRAINT: Clearing drops pending work but cannot recall a request already on the wire.
    cancel(): void {
      waiting = null;
    },

    idle(): Promise<void> {
      return draining ?? Promise.resolve();
    },
  };
}
