// Operator feedback after a run ends, and the lessons it becomes for later runs of the same task.

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button.js";
import { Textarea } from "@/components/ui/textarea.js";
import { MAX_FEEDBACK_LENGTH } from "../../../shared/run-api.js";
import { errorMessage } from "../../errors.js";
import {
  type Lesson,
  type LessonEdits,
  lessonEdit,
  type Run,
} from "../../run-logic.js";
import type { useAgentRun } from "./use-agent-run.js";

type Agent = ReturnType<typeof useAgentRun>;

const LESSON_LOOKUP_DELAY_MILLISECONDS = 300;

// Lessons for the typed task; refreshed when feedback is saved. Until the lookup for the
// current text settles, `ready` is false and no lessons are offered, so a run never starts
// with lessons loaded for a different task.
export function useTaskLessons(
  loadLessons: Agent["loadLessons"],
  task: string,
  enabled: boolean,
  refreshKey: unknown,
): { lessons: Lesson[]; ready: boolean } {
  const [loaded, setLoaded] = useState<{
    task: string;
    lessons: Lesson[];
  } | null>(null);
  const wanted = task.trim();
  const active = enabled && wanted !== "";
  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshKey reloads lessons after feedback is saved
  useEffect(() => {
    if (!active) return;
    let current = true;
    const timer = setTimeout(() => {
      loadLessons(wanted).then(
        (lessons) => {
          if (current) setLoaded({ task: wanted, lessons });
        },
        () => {
          if (current) setLoaded({ task: wanted, lessons: [] });
        },
      );
    }, LESSON_LOOKUP_DELAY_MILLISECONDS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [loadLessons, wanted, active, refreshKey]);
  if (!active) return { lessons: [], ready: true };
  return loaded?.task === wanted
    ? { lessons: loaded.lessons, ready: true }
    : { lessons: [], ready: false };
}

export function LessonsPicker({
  lessons,
  edits,
  onEditsChange,
}: {
  lessons: Lesson[];
  edits: LessonEdits;
  onEditsChange: (edits: LessonEdits) => void;
}) {
  if (lessons.length === 0) return null;
  const enabledCount = lessons.filter(
    (lesson) => lessonEdit(lesson, edits).enabled,
  ).length;
  return (
    <details className="control-options" open>
      <summary>
        Lessons from earlier runs ({enabledCount} of {lessons.length} on)
      </summary>
      <fieldset className="grid gap-3">
        <legend className="sr-only">Lessons from earlier runs</legend>
        <p>
          Operator feedback on earlier runs of this exact task. Edits apply to
          this run only.
        </p>
        {lessons.map((lesson) => {
          const edit = lessonEdit(lesson, edits);
          const id = `agent-lesson-${encodeURIComponent(lesson.runId)}`;
          return (
            <div key={lesson.runId} className="grid gap-1">
              <label htmlFor={`${id}-on`}>
                <input
                  id={`${id}-on`}
                  type="checkbox"
                  checked={edit.enabled}
                  onChange={(event) =>
                    onEditsChange({
                      ...edits,
                      [lesson.runId]: {
                        ...edit,
                        enabled: event.currentTarget.checked,
                      },
                    })
                  }
                />
                After a {lesson.outcome} run,{" "}
                {new Date(lesson.savedAt).toLocaleString()}
              </label>
              <Textarea
                id={`${id}-text`}
                aria-label="Lesson text"
                value={edit.text}
                disabled={!edit.enabled}
                maxLength={MAX_FEEDBACK_LENGTH}
                onChange={(event) =>
                  onEditsChange({
                    ...edits,
                    [lesson.runId]: {
                      ...edit,
                      text: event.currentTarget.value,
                    },
                  })
                }
              />
            </div>
          );
        })}
      </fieldset>
    </details>
  );
}

export function RunFeedback({
  run,
  saveFeedback,
}: {
  run: Run;
  saveFeedback: Agent["saveFeedback"];
}) {
  const saved = run.feedback?.text ?? "";
  const [draft, setDraft] = useState(saved);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState("");
  // A different run, or feedback saved elsewhere, replaces the draft.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset only when the source changes
  useEffect(() => {
    setDraft(saved);
    setStatus("");
  }, [run.id, saved]);
  if (run.phase.state !== "ended") return null;
  const unchanged = draft.trim() === saved;
  return (
    <form
      aria-label="Run feedback"
      className="grid gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        setSaving(true);
        setStatus("");
        saveFeedback(run.id, draft.trim()).then(
          () => {
            setSaving(false);
            setStatus(
              draft.trim()
                ? "Saved. Later runs of this task will offer it as a lesson."
                : "Feedback cleared.",
            );
          },
          (error: unknown) => {
            setSaving(false);
            setStatus(`Could not save feedback: ${errorMessage(error)}`);
          },
        );
      }}
    >
      <label htmlFor="agent-run-feedback">
        Advice for the next run
        <Textarea
          id="agent-run-feedback"
          value={draft}
          disabled={saving}
          maxLength={MAX_FEEDBACK_LENGTH}
          onChange={(event) => setDraft(event.currentTarget.value)}
          placeholder="For example: open the gripper wider before descending"
        />
      </label>
      <Button
        className="w-fit"
        type="submit"
        variant="outline"
        disabled={saving || unchanged}
      >
        {saving ? "Saving…" : "Save feedback"}
      </Button>
      <p className="empty:hidden" role="status">
        {status}
      </p>
    </form>
  );
}
