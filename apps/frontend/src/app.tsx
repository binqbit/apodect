import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import {
  BrowserRouter,
  Link,
  NavLink,
  Navigate,
  Outlet,
  useLocation,
  Route,
  Routes,
} from "react-router-dom";
import { api, ApiError, type Account, type PublicConfig } from "./api";
import type { ConnectedWallet } from "./wallets";
import { Landing } from "./landing";
import { AccountPage, ProjectsPage } from "./pages";
import { ProjectPage } from "./project";
import { Brand, ErrorState, Loading, short } from "./ui";

interface SessionContext {
  account: Account | null;
  config: PublicConfig | null;
  loading: boolean;
  error: unknown;
  wallets: ConnectedWallet[];
  setAccount: (account: Account | null) => void;
  rememberWallet: (wallet: ConnectedWallet) => void;
  refreshAccount: () => Promise<void>;
}
const Session = createContext<SessionContext | null>(null);
export function useSession() {
  const value = useContext(Session);
  if (!value) throw new Error("The application session is not available.");
  return value;
}

function SessionProvider({ children }: { children: ReactNode }) {
  const [account, updateAccount] = useState<Account | null>(null);
  const [config, setConfig] = useState<PublicConfig | null>(null);
  const [wallets, setWallets] = useState<ConnectedWallet[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>();
  const setAccount = useCallback((value: Account | null) => {
    updateAccount(value);
    if (!value) setWallets([]);
  }, []);
  const refreshAccount = useCallback(async () => {
    try {
      setAccount(await api.account());
    } catch (failure) {
      if (failure instanceof ApiError && failure.status === 401)
        setAccount(null);
      else throw failure;
    }
  }, []);
  useEffect(() => {
    let current = true;
    Promise.all([api.config(), refreshAccount()])
      .then(
        ([value]) => {
          if (current) setConfig(value);
        },
        (failure: unknown) => {
          if (current) setError(failure);
        },
      )
      .finally(() => {
        if (current) setLoading(false);
      });
    return () => {
      current = false;
    };
  }, [refreshAccount]);
  const rememberWallet = (wallet: ConnectedWallet) =>
    setWallets((previous) => [
      ...previous.filter(
        (item) =>
          item.chain !== wallet.chain || item.address !== wallet.address,
      ),
      wallet,
    ]);
  return (
    <Session.Provider
      value={{
        account,
        config,
        wallets,
        loading,
        error,
        setAccount,
        rememberWallet,
        refreshAccount,
      }}
    >
      {children}
    </Session.Provider>
  );
}

function WorkspaceLayout() {
  const { account, config } = useSession();
  const location = useLocation();
  return (
    <div className="app-shell">
      <aside className="sidebar">
        <Brand />
        <nav aria-label="Workspace navigation">
          <NavLink
            to="/app"
            end
            className={({ isActive }: { isActive: boolean }) =>
              isActive || location.pathname.startsWith("/app/projects/")
                ? "active"
                : ""
            }
          >
            <span className="nav-icon" aria-hidden="true">
              ▦
            </span>
            Projects
          </NavLink>
          <NavLink to="/app/account">
            <span className="nav-icon" aria-hidden="true">
              ◉
            </span>
            Account & wallets
          </NavLink>
        </nav>
        <div className="sidebar-bottom">
          <Link to="/">How Apodect works ↗</Link>
          <p>
            Your wallet authorizes actions.
            <br />
            Contracts control allocations.
          </p>
        </div>
      </aside>
      <div className="app-main">
        <header className="app-header">
          <div className="header-breadcrumb">
            Workspace <span aria-hidden="true">/</span>
            <strong>Budget coordination</strong>
          </div>
          <Link to="/app/account" className="account-link">
            <span className="avatar" aria-hidden="true">
              {account?.session.profile.display_name
                ?.slice(0, 1)
                .toUpperCase() ?? "◈"}
            </span>
            {account
              ? (account.session.profile.display_name ??
                short(account.session.wallet.address, 5))
              : "Connect wallet"}
          </Link>
        </header>
        {config?.local_simulator && (
          <div className="environment-bar">
            <strong>Local environment</strong>
            <span>Test tokens · Simulated liquidity and MPC providers</span>
          </div>
        )}
        <main className="app-content">
          <Outlet />
        </main>
      </div>
    </div>
  );
}

function Authenticated({ children }: { children: ReactNode }) {
  const { account, loading, error } = useSession();
  if (loading) return <Loading>Connecting to your workspace…</Loading>;
  if (error)
    return <ErrorState error={error} retry={() => location.reload()} />;
  return account ? children : <Navigate to="/app/account" replace />;
}

export function App() {
  return (
    <BrowserRouter>
      <SessionProvider>
        <Routes>
          <Route path="/" element={<Landing />} />
          <Route
            path="/wallet/callback"
            element={
              <div className="container section">
                <Brand />
                <h1 style={{ marginTop: 35 }}>Return to your workspace</h1>
                <p className="muted" style={{ marginTop: 15 }}>
                  The wallet response is handled by the window where you started
                  signing. You can close this window.
                </p>
              </div>
            }
          />
          <Route path="/app" element={<WorkspaceLayout />}>
            <Route
              index
              element={
                <Authenticated>
                  <ProjectsPage />
                </Authenticated>
              }
            />
            <Route path="account" element={<AccountPage />} />
            <Route
              path="projects/:projectKey/*"
              element={
                <Authenticated>
                  <ProjectPage />
                </Authenticated>
              }
            />
          </Route>
          <Route
            path="*"
            element={
              <div className="container section">
                <Brand />
                <h1 style={{ marginTop: 40 }}>Page not found</h1>
                <p className="muted" style={{ margin: "18px 0" }}>
                  This page may have moved. Your projects are available in the
                  workspace.
                </p>
                <Link className="button" to="/app">
                  Go to workspace
                </Link>
              </div>
            }
          />
        </Routes>
      </SessionProvider>
    </BrowserRouter>
  );
}
