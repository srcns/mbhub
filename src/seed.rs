//! Seed corpus: AI-style questions and markdown answers.
//!
//! Each entry has a concise question (<= 80 chars) for the Memory screen list
//! and a rich markdown answer opened in the viewer.

#[derive(Clone, Copy, Debug)]
pub struct SeedEntry {
    pub question: &'static str,
    pub content: &'static str,
}

pub const SEED_ENTRIES: &[SeedEntry] = &[
    SeedEntry {
        question: "When should Arc<Mutex<T>> be used in Rust?",
        content: r#"Use Arc<Mutex<T>> only when shared ownership and interior mutability are both required.

**Rule of thumb:** prefer `Rc<RefCell<T>>` for single-threaded state and plain ownership elsewhere.
- `Arc` = cheap clone, shared across threads
- `Mutex` = exclusive interior mutability
- `Arc<Mutex<T>>` together = the common "shared mutable state" pattern"#,
    },
    SeedEntry {
        question: "How to avoid E0507 borrow errors in spawned threads?",
        content: r#"Clone the value before moving it into a spawned thread to avoid the E0507 borrow error.

**Why:** `thread::spawn` requires a `'static` closure, so any borrowed data must be owned first.
```rust
let s = String::from("hello");
std::thread::spawn(move || {
    println!("{s}"); // ok: s was moved, not borrowed
});
```"#,
    },
    SeedEntry {
        question: "Why must a future be pinned before being polled in Rust?",
        content: r#"A future must be pinned before it can be polled across await points.

**Why:** `Future` values may contain self-references; `Pin` guarantees they do not move.
- `Box::pin` for heap futures
- `pin!` macro for stack futures
- `async fn` returns an opaque type that is already safe to poll"#,
    },
    SeedEntry {
        question: "How to execute CPU-bound work in async Rust?",
        content: r#"Use spawn_blocking for CPU-bound work instead of blocking the async runtime.

**Why:** a busy loop inside `async` starves other tasks because they all share one executor thread.
- `tokio::task::spawn_blocking` for sync/CPU work
- `tokio::task::spawn` for I/O-bound tasks
- never call `std::thread::sleep` inside `async fn`"#,
    },
    SeedEntry {
        question: "When to use thiserror vs anyhow in Rust projects?",
        content: r#"Return typed errors with thiserror in libraries and use anyhow in applications.

**Tradeoff:**
- `thiserror` derives `Display` + `Error` for matchable variants
- `anyhow` boxes errors for ergonomic `?` propagation
- a library should expose typed errors; a binary can use `anyhow` freely"#,
    },
    SeedEntry {
        question: "What is the difference between traits and generics in Rust?",
        content: r#"Traits enable polymorphism while generics are monomorphized into concrete code per type.

**Consequence:** generic functions get duplicated per type (fast, more code) while trait objects use a vtable (slower dispatch, single copy).
```rust
fn print_len<T: AsRef<str>>(s: T) {} // monomorphized
fn print_len_dyn(s: &dyn AsRef<str>) {} // dynamic dispatch
```"#,
    },
    SeedEntry {
        question: "How to structure a multi-crate Cargo workspace?",
        content: r#"Structure a workspace with one crate per concern and a thin public API surface.

**Layout:**
```text
crates/
  core/       # domain types, no I/O
  storage/    # persistence
  cli/        # entry point
```
- internal crates stay unpublished (`publish = false`)
- `cargo run -p cli` builds only what is needed"#,
    },
    SeedEntry {
        question: "How does lifetime elision work in Rust signatures?",
        content: r#"Lifetime elision hides the common cases so signatures stay readable.

**Rules:** one input lifetime maps to the output; multiple inputs require explicit annotations.
```rust
fn first<'a>(x: &'a str, _y: &str) -> &'a str { x }
// elided: fn first<'a>(x: &'a str, _y: &'a str) -> &'a str
```"#,
    },
    SeedEntry {
        question: "How to chain Option and Result cleanly in Rust?",
        content: r#"Chain Option and Result with map/and_then instead of nested matches.

**Style:**
```rust
let user = find_user(id)
    .and_then(|u| u.profile())   // Option
    .ok_or(AppError::Missing)?;  // into Result
```
- `map` transforms the inner value
- `and_then` flattens a nested Option/Result"#,
    },
    SeedEntry {
        question: "Why are Rust iterators lazy and when do they evaluate?",
        content: r#"Iterators are lazy: nothing runs until a consuming adaptor is called.

**Implication:** chaining `map`/`filter` builds a pipeline that executes once at `collect`, `sum`, or `for`.
```rust
let n = (1..=10).filter(|x| x % 2 == 0).count();
```"#,
    },
    SeedEntry {
        question: "Why keep functions pure with immutable inputs?",
        content: r#"Reach for unsafe only at FFI boundaries and wrap it in a safe API.

**Contract:** the caller of `unsafe` must uphold the invariants the compiler no longer checks.
- keep `unsafe` blocks small and documented
- prefer safe wrappers like `CStr` and slice casts
- `cargo geiger` can audit unsafe usage"#,
    },
    SeedEntry {
        question: "How does structural sharing make persistent data fast?",
        content: r#"Unit tests cover one function; integration tests exercise the public API as a user.

**Layout:**
```text
src/lib.rs           # #[cfg(test)] mod tests
tests/api.rs         # uses the crate as a dependency
```
- unit tests have access to private items
- integration tests are black-box and catch regressions in behavior"#,
    },
    SeedEntry {
        question: "How does tail-call optimization prevent stack overflow?",
        content: r#"Run EXPLAIN ANALYZE to find missing indexes on hot query paths.

**What to look for:**
- `Seq Scan` on large tables = candidate for an index
- high `actual time` vs `planned time` = stale statistics
- `-> Nested Loop` with large N = maybe a hash join is better"#,
    },
    SeedEntry {
        question: "Pattern matching vs polymorphism: what are the tradeoffs?",
        content: r#"A B-tree index serves range and equality lookups; a hash index only equality.

**Use:**
- B-tree for `<`, `>`, `BETWEEN`, and ordered scans (Postgres default)
- hash for point lookups when the whole key is compared
- GIN for array/JSON containment and full-text"#,
    },
    SeedEntry {
        question: "Why prefer composition over inheritance in OOP?",
        content: r#"VACUUM reclaims dead tuples so table and index bloat does not grow without bound.

**Why:** Postgres MVCC leaves old row versions after updates; VACUUM marks that space reusable.
- autovacuum runs automatically but can lag under heavy writes
- monitor `pg_stat_user_tables.n_dead_tup`
- manual `VACUUM (ANALYZE)` refreshes planner statistics too"#,
    },
    SeedEntry {
        question: "How does currying differ from partial application?",
        content: r#"Enable WAL mode in SQLite so readers do not block writers.

**Effect:** writes append to a write-ahead log and readers see a consistent snapshot.
```sql
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;
```
- better concurrency than the default rollback journal
- a small `-wal` file appears next to the database"#,
    },
    SeedEntry {
        question: "What is referential transparency in functional code?",
        content: r#"Store small object caches in a Redis hash instead of many flat keys.

**Why:** one `HSET` keeps related fields together and lowers per-key overhead.
```text
HSET user:42 name "Ada" plan "pro"
HGET user:42 plan
```"#,
    },
    SeedEntry {
        question: "How to design idempotent API endpoints safely?",
        content: r#"Isolation levels trade consistency for concurrency; know which one you need.

**Common levels:**
- `READ COMMITTED` (Postgres default): no dirty reads
- `REPEATABLE READ`: stable snapshot for the whole transaction
- `SERIALIZABLE`: full isolation at the cost of aborts/retries"#,
    },
    SeedEntry {
        question: "Why choose event sourcing for audit-heavy domains?",
        content: r#"Pool connections once and reuse them instead of opening one per request.

**Why:** establishing a connection (TCP + auth + session) is expensive.
- cap pool size to what the database can handle
- timeouts prevent a stuck query from exhausting the pool
- `deadpool-postgres`, `r2d2`, `sqlx` pool all follow this model"#,
    },
    SeedEntry {
        question: "How does CQRS separate read and write models?",
        content: r#"Apply schema changes with versioned, forward-only migrations.

**Rules:**
- each migration is one small, reversible-if-possible step
- never edit an applied migration; add a new one
- store a `schema_migrations` table so tooling can track state"#,
    },
    SeedEntry {
        question: "What is a circuit breaker and how does it prevent outages?",
        content: r#"Avoid the N+1 query problem by loading related rows in one batched query.

**Symptom:** one query for the list plus one query per row.
**Fix:**
```text
SELECT * FROM posts WHERE author_id IN (SELECT id FROM authors WHERE ...)
```
- an ORM `include`/`preload` usually fixes it
- watch the query log during development"#,
    },
    SeedEntry {
        question: "Why implement exponential backoff with jitter on retries?",
        content: r#"Normalize to remove redundancy; denormalize only when reads demand it.

**Tension:** 3NF avoids update anomalies but joins get expensive; a read-heavy path may justify a denormalized cache or summary table.
- measure before denormalizing
- keep the canonical form authoritative"#,
    },
    SeedEntry {
        question: "How does rate limiting protect downstream services?",
        content: r#"Full-text search needs an index built for tokens, not raw substring scans.

**Options:**
- Postgres `tsvector` + GIN index
- SQLite FTS5 virtual table
- dedicated engines (Meilisearch, Typesense) for ranking at scale"#,
    },
    SeedEntry {
        question: "Why apply schema changes with forward-only migrations?",
        content: r#"Backups are worthless unless they are tested by restoring them.

**Practice:**
- automate `pg_dump` or file-level snapshots
- store off-site and versioned
- schedule a regular restore drill so recovery is a known procedure, not a theory"#,
    },
    SeedEntry {
        question: "How does write-ahead logging guarantee ACID durability?",
        content: r#"Multi-stage Docker builds keep production images small by discarding build tools.

**Shape:**
```dockerfile
FROM rust AS build
# ... cargo build --release
FROM debian:bookworm-slim
COPY --from=build /app/target/release/app /usr/local/bin/app
```
- final stage has no compiler or source
- `docker history` shows which layer pulls in weight"#,
    },
    SeedEntry {
        question: "B-trees vs LSM-trees: when to use which database engine?",
        content: r#"Liveness and readiness probes answer different questions in Kubernetes.

**Distinction:**
- liveness: "should the container be restarted?"
- readiness: "should traffic be sent to it?"
- a slow startup should fail readiness, not liveness"#,
    },
    SeedEntry {
        question: "How does database connection pooling reduce latency?",
        content: r#"A CI pipeline should run fast feedback first: format, lint, then tests, then build.

**Ordering:**
1. cheap static checks fail in seconds
2. unit tests catch logic errors
3. integration + build are the slow, final gate
- cache dependencies so builds are incremental"#,
    },
    SeedEntry {
        question: "How to prevent database deadlocks through lock ordering?",
        content: r#"Rate limit login endpoints with limit_req_zone to slow brute-force attempts.

**Sketch:**
```nginx
limit_req_zone $binary_remote_addr zone=login:10m rate=5r/m;
location /login { limit_req zone=login burst=3; }
```
- pair with account lockout and CAPTCHA for defense in depth"#,
    },
    SeedEntry {
        question: "How does database sharding scale write throughput?",
        content: r#"Blue-green deployment keeps two environments and swaps traffic between them.

**Benefit:** rollback is instant — point the router back at the old version.
- run both versions side by side
- health-check the new one before the cutover
- handles schema changes carefully (expand/migrate/contract)"#,
    },
    SeedEntry {
        question: "Why use multi-stage Docker builds in production?",
        content: r#"Infrastructure as code makes environments reproducible and reviewable.

**Why:** hand-edited servers drift and cannot be rebuilt reliably.
- Terraform/OpenTofu for resources
- Ansible/Chef for configuration
- store the code in git and review changes like code"#,
    },
    SeedEntry {
        question: "How to design minimal distroless container images?",
        content: r#"Observability is three pillars: metrics, logs, and traces.

**Mapping:**
- metrics: "is something wrong?" (counters, gauges)
- logs: "what happened on one node?" (events)
- traces: "how did one request flow through services?"
- correlate them with a shared request id"#,
    },
    SeedEntry {
        question: "How to optimize Docker build cache layering?",
        content: r#"Never commit secrets; inject them at runtime from a vault or environment.

**Practice:**
- env vars, a secrets manager, or cloud KMS
- rotate keys regularly
- `git log` will remember a leaked secret forever — revoke immediately if one slips in"#,
    },
    SeedEntry {
        question: "Why run container processes as non-root users?",
        content: r#"A load balancer spreads traffic with a strategy: round-robin, least-connections, or hashing.

**Choosing:**
- round-robin: even, stateless distribution
- least-connections: better when requests vary in cost
- consistent hashing: sticky routing for caches/sessions"#,
    },
    SeedEntry {
        question: "Difference between Kubernetes liveness and readiness probes?",
        content: r#"Containers share a kernel; VMs virtualize the hardware.

**Tradeoff:**
- containers: fast start, small footprint, shared host kernel
- VMs: stronger isolation, heavier, per-guest OS
- many platforms nest containers inside VMs for both speed and isolation"#,
    },
    SeedEntry {
        question: "How to configure Kubernetes graceful termination periods?",
        content: r#"Squash commits with an interactive rebase before opening a pull request.

**Steps:**
```text
git rebase -i main
# mark fixups as 's' (squash) or 'f' (fixup)
```
- one logical change per commit
- force-push only to your own branch"#,
    },
    SeedEntry {
        question: "What are the core pillars of distributed observability?",
        content: r#"Semantic versioning encodes breaking vs additive vs patch changes.

**Format:** `MAJOR.MINOR.PATCH`
- bump MAJOR on incompatible API changes
- bump MINOR on backward-compatible features
- bump PATCH on bug fixes
- `0.x` releases are allowed to break anything"#,
    },
    SeedEntry {
        question: "Why are distributed trace IDs essential for microservices?",
        content: r#"Discriminated unions make impossible states unrepresentable in TypeScript.

**Pattern:**
```ts
type Result<T> =
  | { ok: true; value: T }
  | { ok: false; error: string };
```
- the compiler exhaustively checks every branch
- eliminates the `undefined` guessing game"#,
    },
    SeedEntry {
        question: "How to compute high-percentile latency p99 accurately?",
        content: r#"Memoize expensive selectors so a state change does not re-run heavy derivations.

**Why:** recomputing a filtered/sorted list on every render wastes CPU.
- `useMemo` for values, `useCallback` for functions
- keep deps correct or memoization silently breaks"#,
    },
    SeedEntry {
        question: "How does continuous profiling identify performance hotspots?",
        content: r#"WebSocket is bidirectional and stateful; SSE is a one-way stream from server to client.

**Choose:**
- SSE: notifications, feeds, simple server push over HTTP
- WebSocket: chat, games, anything needing client→server frames
- SSE survives proxies and reconnects more gracefully"#,
    },
    SeedEntry {
        question: "What is the difference between latency, errors, and saturation?",
        content: r#"Flexbox lays out in one dimension; Grid lays out in two.

**Rule of thumb:**
- a row or column of items → flexbox
- a page skeleton with rows and columns → grid
- both can nest; do not force one to do the other's job"#,
    },
    SeedEntry {
        question: "How to defend against prompt injection in LLM apps?",
        content: r#"A virtual DOM reconciles a lightweight tree description against the real DOM.

**Benefit:** you describe the target UI and the framework computes minimal changes.
- diffing is O(n) with keys
- stable `key` props prevent list churn bugs"#,
    },
    SeedEntry {
        question: "What is retrieval-augmented generation (RAG)?",
        content: r#"Lift state to the closest common ancestor that needs it.

**Guideline:** local state stays local; shared state rises only as far as required.
- prop drilling is fine for shallow trees
- reach for a store when many distant components read the same data"#,
    },
    SeedEntry {
        question: "How does sampling temperature affect LLM randomness?",
        content: r#"Accessibility is not an add-on: use semantic elements and ARIA where needed.

**Basics:**
- real `<button>` instead of clickable `<div>`
- `alt` text for images, labels for inputs
- keyboard focus order must match visual order"#,
    },
    SeedEntry {
        question: "Why is semantic caching effective for LLM inference?",
        content: r#"Code-split at route boundaries so users download only what they visit.

**Effect:** smaller initial bundle, faster first paint.
- dynamic `import()` for routes and heavy libraries
- keep a shared vendor chunk for common dependencies"#,
    },
    SeedEntry {
        question: "Fine-tuning vs RAG: when to choose which approach?",
        content: r#"Hydration attaches event listeners to server-rendered HTML without re-rendering.

**Pitfall:** server and client markup must match or React throws a hydration mismatch.
- avoid `Date.now()` and random values during render
- defer non-deterministic UI to effects"#,
    },
    SeedEntry {
        question: "What is model overfitting and how to prevent it?",
        content: r#"CORS is a browser-only policy enforced by the client, not by the server.

**Mechanism:** the server declares which origins may read responses via `Access-Control-Allow-Origin`.
- preflight `OPTIONS` for non-simple requests
- same-origin requests are unaffected"#,
    },
    SeedEntry {
        question: "Why are embeddings effective for semantic search?",
        content: r#"Add __slots__ to a dataclass to cut memory for large object graphs.

**Why:** `__slots__` replaces the per-instance `__dict__` with fixed fields.
```python
@dataclass(slots=True)
class Point:
    x: float
    y: float
```
- prevents adding arbitrary attributes (usually a feature)"#,
    },
    SeedEntry {
        question: "How does speculative decoding speed up inference?",
        content: r#"Context managers guarantee cleanup with a with block.

**Use cases:** files, locks, transactions, timers.
```python
with open("f.txt") as f:
    data = f.read()
# f is closed even on exception
```"#,
    },
    SeedEntry {
        question: "How to quantify quantifiable impact on a technical resume?",
        content: r#"A list comprehension is idiomatic; use map only with an existing function.

**Prefer:**
```python
squares = [x * x for x in range(10)]
```
- comprehensions read better for small transforms
- `map(str.strip, lines)` is clean when reusing a function"#,
    },
    SeedEntry {
        question: "How to effectively negotiate engineering compensation?",
        content: r#"asyncio is cooperative: a blocking call freezes the whole event loop.

**Rule:** never call `time.sleep` or blocking I/O inside a coroutine.
- `await asyncio.sleep` yields control
- offload CPU work to a thread/process executor"#,
    },
    SeedEntry {
        question: "How to write clear and actionable technical emails?",
        content: r#"Vectorized pandas operations beat row-by-row Python loops by orders of magnitude.

**Prefer:**
```python
df["total"] = df["price"] * df["qty"]   # vectorized
```
- `.apply` is a last resort
- `.itertuples()` is faster than `.iterrows()` when a loop is unavoidable"#,
    },
    SeedEntry {
        question: "What are the principles of effective 1-on-1 meetings?",
        content: r#"Always create a virtual environment per project to isolate dependencies.

**Why:** system Python changes can break your app, and two projects may pin different versions.
```text
python -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
```"#,
    },
    SeedEntry {
        question: "How to conduct productive code reviews without blocking?",
        content: r#"A decorator wraps a function to add behavior without editing its body.

**Mechanism:** it receives the function and returns a replacement.
```python
def logged(fn):
    def wrapper(*a, **k):
        print("call", fn.__name__)
        return fn(*a, **k)
    return wrapper
```"#,
    },
    SeedEntry {
        question: "How to write actionable postmortems without blame?",
        content: r#"Generators yield lazily so you can stream data that never fits in memory.

**Contrast:**
```python
def lines():
    for chunk in read_chunks():
        for line in chunk.splitlines():
            yield line
```
- one item at a time, constant memory
- a list would materialize everything at once"#,
    },
    SeedEntry {
        question: "What is the Zettelkasten note-taking method?",
        content: r#"Type hints document intent and let mypy catch bugs before runtime.

**Practice:**
- annotate public function signatures
- run mypy in CI with `--strict` where possible
- hints do not affect runtime but enable tooling"#,
    },
    SeedEntry {
        question: "How to build a sustainable daily reading habit?",
        content: r#"Catch specific exceptions, not bare except, and never swallow errors silently.

**Guideline:**
```python
except ValueError as e:
    log.error("bad input: %s", e)
    raise
```
- re-raise unless you truly handle the condition
- `except Exception` hides bugs like `KeyboardInterrupt` misclassification"#,
    },
    SeedEntry {
        question: "How to maintain deep work focus and avoid distractions?",
        content: r#"A TLS handshake negotiates a cipher suite and exchanges keys before any data flows.

**Rough steps:**
1. client hello (supported ciphers, random)
2. server hello + certificate
3. key exchange (e.g., X25519)
4. both sides derive session keys and confirm with finished messages"#,
    },
    SeedEntry {
        question: "What is the difference between reversible and irreversible choices?",
        content: r#"A Merkle tree proves integrity of a large dataset with a single root hash.

**Why it matters for P2P:** a peer can verify one branch without downloading everything, and tampering changes the root.
- leaves are content hashes
- inner nodes hash their children
- inclusion proofs are O(log n)"#,
    },
    SeedEntry {
        question: "What are the core principles of lifestyle minimalism?",
        content: r#"SHA-256 and BLAKE3 both hash data; BLAKE3 is faster and parallel-friendly.

**Tradeoff:**
- SHA-256: ubiquitous, hardware-accelerated, conservative
- BLAKE3: modern, streaming + parallelism, not yet a FIPS standard
- for local integrity checks BLAKE3 is a strong default"#,
    },
    SeedEntry {
        question: "How does spaced repetition improve long-term retention?",
        content: r#"Defend logins in layers: rate limiting, lockout, and strong password policy.

**Why one is not enough:** rate limiting slows bots, lockout stops credential stuffing, and policy raises the cost per guess.
- store only password hashes (argon2id/bcrypt)
- monitor for unusual login patterns"#,
    },
    SeedEntry {
        question: "How to calculate big-O time and space complexity?",
        content: r#"JWTs are signed, not encrypted; anyone can read their contents.

**Consequence:** never put secrets in a JWT, and verify signature + expiry on every request.
- sessions vs JWT: sessions revoke server-side easily
- short-lived access tokens + refresh tokens is a common pattern"#,
    },
    SeedEntry {
        question: "How does binary search achieve logarithmic runtime?",
        content: r#"Prevent SQL injection by using parameterized queries everywhere.

**Never:**
```python
f"SELECT * FROM users WHERE id = {user_input}"
```
**Always:**
```text
SELECT * FROM users WHERE id = ?
```
- the driver escapes the value, so structure and data stay separate"#,
    },
    SeedEntry {
        question: "How do hash table collision resolution techniques work?",
        content: r#"Prevent XSS by treating all user input as data, not markup.

**Rules:**
- escape output in the correct context (HTML, attribute, JS)
- avoid `innerHTML` with user data
- a strict Content-Security-Policy is a strong second layer"#,
    },
    SeedEntry {
        question: "How do balanced search trees keep operations logarithmic?",
        content: r#"CSRF tokens stop a malicious site from making authenticated requests on your behalf.

**Mechanism:** the server issues an unguessable token that must accompany state-changing requests.
- SameSite cookies also help
- stateless APIs often use custom headers instead"#,
    },
    SeedEntry {
        question: "What is dynamic programming and memoization?",
        content: r#"OAuth2 lets a user grant limited access without sharing their password.

**Flow (authorization code):**
1. redirect to the provider
2. user approves scopes
3. exchange the code for tokens
- use PKCE for public clients"#,
    },
    SeedEntry {
        question: "How does Dijkstra algorithm find shortest paths in graphs?",
        content: r#"Zero trust assumes the network is hostile and verifies every request.

**Principles:**
- authenticate and authorize each call, even internal ones
- least-privilege access, short-lived credentials
- assume breach and segment accordingly"#,
    },
    SeedEntry {
        question: "How does a Bloom filter achieve constant-time membership test?",
        content: r#"DDoS mitigation combines capacity, filtering, and rate limiting at the edge.

**Layers:**
- anycast + CDN absorb volumetric floods
- protocol validation drops malformed packets
- application rules limit expensive endpoints"#,
    },
    SeedEntry {
        question: "How do topological sorting algorithms handle DAGs?",
        content: r#"Encrypt data in transit (TLS) and at rest (disk/database encryption).

**Scope:** transit protects against interception; at-rest protects against a stolen disk.
- manage keys separately from the data they protect
- backups are encrypted too, not just the live database"#,
    },
    SeedEntry {
        question: "What is the union-find disjoint set data structure?",
        content: r#"Big-O describes how work grows with input size, ignoring constant factors.

**Examples:**
- O(1): hash lookup (average)
- O(log n): binary search
- O(n log n): comparison sort lower bound
- O(n^2): nested loops over the same input"#,
    },
    SeedEntry {
        question: "How does a trie prefix tree accelerate autocomplete?",
        content: r#"Hash table collisions are resolved by chaining or open addressing.

**Tradeoff:**
- chaining: linked buckets, tolerates many collisions
- open addressing: probes within the table, cache-friendly
- a good hash + load factor < ~0.7 keeps lookups O(1)"#,
    },
    SeedEntry {
        question: "How does TCP 3-way handshake establish reliable connections?",
        content: r#"Binary search halves the search space each step on sorted data.

**Invariant:** compare the middle element, then discard half.
```text
lo = 0, hi = n-1
while lo <= hi: mid = (lo+hi)//2 ...
```
- O(log n) time, O(1) extra space"#,
    },
    SeedEntry {
        question: "What is the difference between TCP and UDP protocols?",
        content: r#"Quicksort is fast in practice; mergesort is stable and predictable.

**Contrast:**
- quicksort: in-place, cache-friendly, worst case O(n^2) without care
- mergesort: O(n log n) always, stable, but needs O(n) extra space
- hybrid sorts (introsort) avoid quicksort's worst case"#,
    },
    SeedEntry {
        question: "How does TLS 1.3 handshake encrypt web traffic?",
        content: r#"Dijkstra finds shortest paths on graphs with non-negative edge weights.

**Mechanism:** a priority queue repeatedly relaxes the closest unvisited node.
- O((V+E) log V) with a binary heap
- negative edges require Bellman-Ford instead"#,
    },
    SeedEntry {
        question: "How does HTTP/2 multiplexing eliminate head-of-line blocking?",
        content: r#"Dynamic programming solves overlapping subproblems by memoizing their answers.

**Signs:** optimal substructure + repeated subproblems.
```text
fib(n) = fib(n-1) + fib(n-2)  # top-down memo or bottom-up table
```
- turns exponential recursion into polynomial time"#,
    },
    SeedEntry {
        question: "How does DNS hierarchical resolution map domain names to IPs?",
        content: r#"BFS finds shortest paths in unweighted graphs; DFS explores deeply first.

**Use:**
- BFS: shortest path, level order (uses a queue)
- DFS: cycle detection, topological sort, backtracking (uses a stack/recursion)
- both run in O(V + E)"#,
    },
    SeedEntry {
        question: "What is CORS and why is it client-enforced by browsers?",
        content: r#"An LRU cache evicts the least-recently-used entry when full.

**Implementation:** a hash map for O(1) lookup plus a doubly-linked list for order.
- ideal for caches with temporal locality
- the same idea powers OS page replacement"#,
    },
    SeedEntry {
        question: "How do WebSockets provide full-duplex client-server communication?",
        content: r#"Deep recursion can overflow the stack; convert to iteration or an explicit stack.

**Why:** each call frame consumes stack, which is bounded.
- tail recursion is optimized only in some languages (not Python/JS by default)
- iterative DFS with an explicit stack avoids the limit"#,
    },
    SeedEntry {
        question: "How does BGP routing govern inter-autonomous system traffic?",
        content: r#"A balanced search tree keeps height O(log n) so all operations stay logarithmic.

**Examples:** AVL (strict), red-black (relaxed, fewer rotations), B-trees (wide nodes for disk).
- an unbalanced tree degrades to a linked list (O(n))
- databases use B-trees to minimize disk seeks"#,
    },
    SeedEntry {
        question: "What is QUIC protocol and why does it run over UDP?",
        content: r#"Vector embeddings map text or objects into points where similarity is distance.

**Key property:** semantically similar items land close together.
- cosine similarity measures angle, not magnitude
- embeddings power search, clustering, and RAG
- dimension + training data determine quality"#,
    },
    SeedEntry {
        question: "How does CDN edge caching reduce origin server load?",
        content: r#"RAG retrieves relevant context first, then asks the model to answer from it.

**Pipeline:** embed the query → search a vector store → inject top hits into the prompt.
- grounds answers in your data
- reduces hallucination and keeps knowledge up to date"#,
    },
    SeedEntry {
        question: "How to write effective unit tests using AAA pattern?",
        content: r#"Attention lets a model weigh which tokens matter for each prediction.

**Mechanism:** queries attend to keys and produce weighted values.
- self-attention relates tokens within a sequence
- it is the core of transformers and scales to long context"#,
    },
    SeedEntry {
        question: "What are the differences between mocks, stubs, and fakes?",
        content: r#"Overfitting means the model memorized training noise instead of the pattern.

**Fixes:**
- more data or data augmentation
- regularization (dropout, weight decay)
- early stopping and a held-out validation set"#,
    },
    SeedEntry {
        question: "What is property-based testing and fuzzing?",
        content: r#"Temperature controls randomness in sampling from a model's output distribution.

**Effect:** lower temperature = more deterministic, higher = more varied.
- 0 (greedy) picks the top token
- ~1 samples proportionally
- high temperature risks incoherence"#,
    },
    SeedEntry {
        question: "How to design end-to-end smoke test suites?",
        content: r#"A context window is the maximum input the model can attend to at once.

**Practical notes:**
- measured in tokens, not characters
- overflow requires truncation or chunking
- larger windows raise cost and can dilute attention"#,
    },
    SeedEntry {
        question: "Why should integration tests run against real ephemeral DBs?",
        content: r#"Fine-tuning changes model weights; prompting changes only the input.

**Choose:**
- prompting: zero infra, iterate instantly
- fine-tuning: bakes in style/domain knowledge, needs data + compute
- RAG sits between them: external knowledge without retraining"#,
    },
    SeedEntry {
        question: "What is regression testing and when should it run?",
        content: r#"Cosine similarity ignores magnitude, making it robust for text embeddings.

**Formula:** dot product divided by the product of norms, range [-1, 1].
- 1 = same direction, 0 = orthogonal, -1 = opposite
- works even when documents differ in length"#,
    },
    SeedEntry {
        question: "How does mutation testing evaluate test suite quality?",
        content: r#"Transformers replaced recurrent networks because they parallelize across sequence.

**Contrast:**
- LSTM/RNN: process tokens one by one, struggle with long-range memory
- transformer: attends to all positions at once, scales with hardware
- positional encodings preserve order in transformers"#,
    },
    SeedEntry {
        question: "What is continuous integration and automated gating?",
        content: r#"Tokenization splits text into units the model actually consumes.

**Kinds:** word, subword (BPE), and character tokens.
- subword handles rare words gracefully
- token count, not character count, drives cost and context limits"#,
    },
    SeedEntry {
        question: "What are the tradeoffs of 100% test coverage targets?",
        content: r#"Entropy measures how spread out a probability distribution is.

**Intuition:** a fair coin has more entropy than a biased one; the uniform distribution maximizes it.
- Shannon entropy: H = -Σ p log p
- low entropy = predictable, compressible data"#,
    },
    SeedEntry {
        question: "How to test asynchronous distributed systems reliably?",
        content: r#"Matrix multiplication composes linear transformations.

**Key idea:** `(AB)x = A(Bx)`, so multiplication chains transformations right-to-left.
- O(n^3) naive; Strassen and beyond shave the exponent
- matrix multiply dominates deep learning compute"#,
    },
    SeedEntry {
        question: "What is the principle of least privilege in security?",
        content: r#"Bayes' theorem updates a belief when new evidence arrives.

**Formula:** P(H|E) = P(E|H)·P(H) / P(E).
- the prior is your starting belief
- the likelihood is how expected the evidence is under the hypothesis"#,
    },
    SeedEntry {
        question: "Why is zero trust architecture superior to perimeter defense?",
        content: r#"A probability distribution assigns mass to outcomes; its shape dictates behavior.

**Common ones:**
- normal: bell curve, sums of many small effects
- Poisson: counts of rare events
- exponential: waiting times between events"#,
    },
    SeedEntry {
        question: "How does public-key asymmetric encryption work?",
        content: r#"Prime numbers are the atoms of integers under multiplication.

**Facts:**
- every integer factors uniquely into primes
- primality testing is fast; factoring is believed hard
- this asymmetry underpins RSA and much of modern crypto"#,
    },
    SeedEntry {
        question: "How does salting and hashing protect stored passwords?",
        content: r#"A limit describes where a function heads as the input approaches a point.

**Intuition:** `f(x) → L` means values get arbitrarily close to L, not necessarily equal.
- continuity = the limit equals the function value
- derivatives are limits of slopes"#,
    },
    SeedEntry {
        question: "How does OAuth2 token authorization protect credentials?",
        content: r#"Energy is conserved in closed systems; entropy only increases.

**Everyday meaning:** useful energy degrades into heat, which is why perpetual motion is impossible.
- the second law of thermodynamics is statistical, not a force
- living things maintain order by exporting entropy"#,
    },
    SeedEntry {
        question: "What is Cross-Site Scripting (XSS) and how to prevent it?",
        content: r#"A qubit is a two-level quantum system that can be in a superposition.

**Properties:** superposition, entanglement, and interference.
- measurement collapses the state
- a useful quantum computer needs error correction, not just more qubits"#,
    },
    SeedEntry {
        question: "How to defend web applications against SQL injection?",
        content: r#"Time dilation means moving clocks tick slower relative to a stationary observer.

**Relativity:** the effect is negligible at everyday speeds but real for fast particles.
- GPS must correct for both special and general relativistic shifts
- at light speed time would stand still (unreachable for mass)"#,
    },
    SeedEntry {
        question: "What is Cross-Site Request Forgery (CSRF) protection?",
        content: r#"Natural selection is the non-random survival of heritable variation.

**Ingredients:** variation, inheritance, and differential reproductive success.
- selection acts on phenotypes, evolution changes gene frequencies
- it has no foresight or goal"#,
    },
    SeedEntry {
        question: "Why is multi-factor authentication (MFA) necessary?",
        content: r#"The Pomodoro technique breaks work into 25-minute focused sprints with short breaks.

**Rhythm:**
- 25 min deep work, 5 min rest
- after 4 sprints take a longer break
- the point is starting, not the exact timer length"#,
    },
    SeedEntry {
        question: "How does a Content Security Policy (CSP) stop script attacks?",
        content: r#"Zettelkasten note-taking links small atomic notes instead of filing by category.

**Practice:**
- one idea per note, written in your own words
- link related notes
- structure emerges from the links, not from folders"#,
    },
    SeedEntry {
        question: "What is a decorator pattern and how does it wrap functions?",
        content: r#"Deep work means long, distraction-free blocks on cognitively demanding tasks.

**Why it matters:** context switching destroys the concentration needed for hard problems.
- schedule it like a meeting
- remove notifications and set availability"#,
    },
    SeedEntry {
        question: "What is the factory pattern and when should it be used?",
        content: r#"A resume should show impact, not just duties: quantify what you changed.

**Formula:** verb + task + measurable result.
- "cut build time 40% by caching dependencies" beats "responsible for CI"
- tailor the top third to the specific role"#,
    },
    SeedEntry {
        question: "What is the observer pattern and how does pub-sub work?",
        content: r#"Salary negotiation is expected, not rude; anchor with researched market data.

**Tips:**
- know the range before you state a number
- negotiate total compensation, not just base
- be ready to walk away politely"#,
    },
    SeedEntry {
        question: "What is the strategy pattern and how does it swap algorithms?",
        content: r#"Dollar-cost averaging smooths market timing risk by investing fixed amounts on schedule.

**Benefit:** you buy more shares when prices fall and fewer when they rise.
- removes the need to predict tops and bottoms
- works best with a long time horizon"#,
    },
    SeedEntry {
        question: "What is the adapter pattern and how does it bridge interfaces?",
        content: r#"Compound interest rewards time more than rate: small early savings grow enormously.

**Example:** money doubles roughly every 72/r years (rule of 72).
- start early, even with small amounts
- fees compound against you just as surely"#,
    },
    SeedEntry {
        question: "What is the singleton pattern and why is it often an anti-pattern?",
        content: r#"Sleep is when memory consolidates and the brain clears metabolic waste.

**Hygiene:**
- consistent schedule, even weekends
- dark, cool room; screens off before bed
- caffeine has a long half-life — stop it early"#,
    },
    SeedEntry {
        question: "What is the command pattern and how does it enable undo/redo?",
        content: r#"Progressive overload drives strength: gradually add weight, reps, or sets.

**Principle:** the body adapts to stress, so the stress must slowly increase.
- compound lifts (squat, deadlift, press) give the most per session
- rest and protein are where growth actually happens"#,
    },
    SeedEntry {
        question: "What is the proxy pattern and how does it control object access?",
        content: r#"Hydration needs vary with activity and heat; thirst is a decent guide for most people.

**Practical:**
- pale yellow urine ≈ hydrated
- drink more before/during exercise
- food also contributes water, not just beverages"#,
    },
    SeedEntry {
        question: "What is the builder pattern and when is it preferable to constructors?",
        content: r#"Coffee extraction depends on grind size, ratio, and water temperature.

**Starting point:**
- ratio ~1:16 coffee to water by weight
- water ~92–96 °C
- grind finer if sour, coarser if bitter"#,
    },
    SeedEntry {
        question: "What is the iterator pattern and how does it decouple traversal?",
        content: r#"Sourdough rises from wild yeast and bacteria in a living starter.

**Key steps:** feed the starter, build dough strength, long cold proof for flavor.
- patience matters more than precision
- a healthy starter doubles predictably after feeding"#,
    },
    SeedEntry {
        question: "How to write semantic HTML for accessibility compliance?",
        content: r#"Writers can version prose with git just like code: commit drafts and branches.

**Workflow:**
- one branch per piece or revision
- commit messages describe what changed and why
- diff shows exactly what you edited between drafts"#,
    },
    SeedEntry {
        question: "How to optimize web vitals (LCP, FID, CLS) on frontend?",
        content: r#"Keyboard shortcuts pay off in the applications you use every day.

**High value:**
- cmd/ctrl + arrows to jump words and lines
- search-in-app (cmd/ctrl + f or k)
- learn the 5–10 you touch constantly, then add more"#,
    },
    SeedEntry {
        question: "What is the CSS box model and how does box-sizing work?",
        content: r#"Email etiquette favors a clear subject, one ask, and short paragraphs.

**Guideline:**
- put the request or decision in the first line
- be specific about deadlines
- reply-all only when everyone truly needs it"#,
    },
    SeedEntry {
        question: "How does CSS flexbox layout distribute space dynamically?",
        content: r#"Remote work needs written communication by default because hallway context is gone.

**Practices:**
- over-communicate decisions in writing
- async-first, but protect a few synchronous hours
- document, so knowledge is not trapped in one head"#,
    },
    SeedEntry {
        question: "How does CSS grid layout create two-dimensional web grids?",
        content: r#"Language learning works best with daily exposure plus deliberate practice.

**Recipe:**
- lots of comprehensible input (listening/reading)
- speak and write early, accept mistakes
- spaced repetition for vocabulary"#,
    },
    SeedEntry {
        question: "What is the virtual DOM and how does reconciliation work?",
        content: r#"Memory techniques turn abstract facts into vivid, spatial images.

**Tools:**
- memory palace: place items along a familiar route
- chunking: group digits/items
- retrieval practice beats re-reading"#,
    },
    SeedEntry {
        question: "How does client-side routing work in Single Page Apps (SPA)?",
        content: r#"Good decisions separate reversible from irreversible choices.

**Framework:**
- reversible decisions: decide fast, learn, adjust
- irreversible ones: gather data, seek a second opinion
- write down the reasoning so you can audit it later"#,
    },
    SeedEntry {
        question: "What is Server-Side Rendering (SSR) vs Static Site Gen (SSG)?",
        content: r#"Minimalism is removing what does not serve you so the rest gets attention.

**Not about counting objects:** it is about reducing decision load and maintenance.
- start with one category (clothes, inbox, commitments)
- keep what you use and what genuinely matters"#,
    },
    SeedEntry {
        question: "How does browser service worker caching enable PWA offline mode?",
        content: r#"Time blocking assigns tasks to calendar slots instead of a running to-do list.

**Why:** a task without a time slot is a hope, not a plan.
- block deep work first
- leave buffer between blocks for the unexpected"#,
    },
    SeedEntry {
        question: "How to implement accessible ARIA roles and keyboard navigation?",
        content: r#"Use semantic HTML where possible; add aria-* attributes to bridge dynamic states.

**Key principles:**
- Ensure every interactive element is reachable via Tab and activatable via Enter/Space
- Use aria-expanded and aria-hidden to reflect dynamic disclosure states
- Never remove focus outlines without providing an accessible high-contrast alternative"#,
    },
];