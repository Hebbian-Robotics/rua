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
        try {
          onError(error);
        } catch (reportError) {
          console.error("Could not report request failure", reportError);
        }
      }
    }
    draining = null;
  }

  return {
    push(value: Value): void {
      waiting = { value };
      if (draining === null) {
        let completeDrain: () => void = () => {};
        draining = new Promise<void>((resolve) => {
          completeDrain = resolve;
        });
        void drain().then(completeDrain);
      }
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
