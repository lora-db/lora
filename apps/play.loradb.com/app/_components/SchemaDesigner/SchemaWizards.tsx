"use client";

/**
 * `SchemaWizards` — global mount point for the New-Index and
 * New-Constraint wizard modals. Mounted once at the workbench level
 * so any sidebar row, recommendation card, or notification can open a
 * wizard regardless of which sidebar surface is currently active.
 *
 * The wizard children are mounted only while their modal is open so
 * their internal step / draft state resets cleanly between sessions.
 */

import { useEffect } from "react";
import { Modal } from "@mantine/core";

import { useStore } from "@/lib/state/store";
import { refreshSchemaDesign } from "@/lib/actions/schemaDesignActions";
import { refreshSchema } from "@/lib/actions/schemaActions";

import { NewConstraintWizard } from "./Wizards/NewConstraintWizard";
import { NewIndexWizard } from "./Wizards/NewIndexWizard";

export function SchemaWizards() {
  const wizard = useStore((s) => s.wizard);
  const editingIndex = useStore((s) => s.editingIndexDef);
  const editingConstraint = useStore((s) => s.editingConstraintDef);
  const close = useStore((s) => s.closeWizard);

  // Whenever a wizard opens, make sure the design catalog and the
  // schema-introspection cache used by the label / property pickers
  // are both fresh.
  useEffect(() => {
    if (wizard === null) return;
    void refreshSchemaDesign();
    void refreshSchema();
  }, [wizard]);

  return (
    <>
      <Modal
        opened={wizard === "newIndex"}
        onClose={close}
        title={editingIndex ? `Edit index — ${editingIndex.name}` : "New index"}
        size="xl"
        centered
        keepMounted={false}
      >
        {wizard === "newIndex" && <NewIndexWizard onClose={close} />}
      </Modal>
      <Modal
        opened={wizard === "newConstraint"}
        onClose={close}
        title={
          editingConstraint
            ? `Edit constraint — ${editingConstraint.name}`
            : "New constraint"
        }
        size="xl"
        centered
        keepMounted={false}
      >
        {wizard === "newConstraint" && <NewConstraintWizard onClose={close} />}
      </Modal>
    </>
  );
}
