import { createRoot } from 'react-dom/client';
import App from './App';
import { createClient } from './lib/client';
import './styles.css';

// One client for the whole app lifetime (WebSocket or VITE_MOCK=1 fixtures).
const client = createClient();

// Surface which transport this build runs against (the same honest string
// diagnostics reports). The browser regression suite refuses to drive
// anything but the mock transport — this is its guard rail against silently
// attaching to a dev server that talks to a real daemon.
document.documentElement.dataset.jeliyaTransport = client.describe();

createRoot(document.getElementById('root')!).render(<App client={client} />);
