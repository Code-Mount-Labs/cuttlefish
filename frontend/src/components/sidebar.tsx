import { useState, type ReactNode } from "react";
import { PanelLeft } from "lucide-react";
import { useMobile } from "../hooks/use-mobile";
import { Button } from "./ui/button";
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "./ui/dialog";
export function Sidebar({ children }: { children: ReactNode }) {
  const mobile = useMobile(),
    [open, setOpen] = useState(false);
  if (!mobile)
    return (
      <aside className="sidebar" aria-label="Navigation">
        {children}
      </aside>
    );
  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button
          id="mobile-menu"
          className="mobile-menu"
          variant="ghost"
          size="icon"
          aria-label="Open navigation"
        >
          <PanelLeft />
        </Button>
      </DialogTrigger>
      <DialogContent className="mobile-sidebar" aria-describedby={undefined}>
        <DialogTitle className="sr-only">Navigation</DialogTitle>
        <div
          role="presentation"
          onClick={(e) => {
            if ((e.target as HTMLElement).closest("[data-navigate]"))
              setOpen(false);
          }}
        >
          {children}
        </div>
      </DialogContent>
    </Dialog>
  );
}
