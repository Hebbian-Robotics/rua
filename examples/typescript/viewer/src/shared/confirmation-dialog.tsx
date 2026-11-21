import { type ReactElement, useRef } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog.js";

export function ConfirmationDialog({
  trigger,
  title,
  description,
  actionLabel,
  actionDisabled = false,
  onConfirm,
}: {
  trigger: ReactElement;
  title: string;
  description: string;
  actionLabel: string;
  actionDisabled?: boolean;
  onConfirm: () => void;
}) {
  const cancelButton = useRef<HTMLButtonElement>(null);

  return (
    <AlertDialog>
      <AlertDialogTrigger render={trigger} />
      <AlertDialogContent initialFocus={cancelButton}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel ref={cancelButton}>Cancel</AlertDialogCancel>
          <AlertDialogAction disabled={actionDisabled} onClick={onConfirm}>
            {actionLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
