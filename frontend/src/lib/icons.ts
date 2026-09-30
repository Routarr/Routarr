/**
 * One place for the icons, and for the ones Lucide renames between versions.
 *
 * `@lucide/svelte` exports no aliases, so importing a retired name fails
 * loudly, which is fine. The dangerous one is `History`: the obvious guess is
 * `Clock`, and it is the wrong drawing. The right one is `RotateCcwClock`,
 * whose path data draws the retired `History`.
 *
 * Re-exported under the names the screens use, so a rename is stated here once
 * instead of in every screen.
 */
export {
  Activity,
  Archive,
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Ban,
  BookOpen,
  Check,
  Circle,
  CircleMinus,
  CirclePlus,
  ClipboardCheck,
  Compass,
  Copy,
  Download,
  ExternalLink,
  Film,
  FlaskConical,
  FolderTree,
  GitFork,
  Info,
  KeyRound,
  Layers,
  LayoutDashboard,
  Link2,
  ListChecks,
  Lock,
  Menu,
  MoveDown,
  MoveUp,
  Pencil,
  Play,
  Plug,
  Plus,
  RefreshCw,
  Save,
  ScrollText,
  Search,
  Server,
  Settings,
  ShieldAlert,
  ShieldCheck,
  Trash2,
  Tv,
  Undo2,
  Upload,
  Wifi,
  X,
} from '@lucide/svelte';

export {
  TriangleAlert as AlertTriangle,
  CircleCheckBig as CheckCircle2,
  CircleQuestionMark as HelpCircle,
  RotateCcwClock as History,
  LoaderCircle as Loader2,
  Ellipsis as MoreHorizontal,
} from '@lucide/svelte';
