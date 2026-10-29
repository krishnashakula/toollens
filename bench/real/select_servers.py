"""Builds the real-world catalog: vendor-published MCP servers, verified and sandboxed.

    python bench/real/select_servers.py verify             # -> servers.json (network: npm + PyPI)
    python bench/real/select_servers.py config [report]    # -> gateway.json (+ learned.json)
    python bench/real/select_servers.py finalize report    # -> catalog.json (indexed servers only)

A package is kept only if its registry metadata links to the vendor's own GitHub org, or it is
published under the vendor's own npm scope (a claimed npm org scope cannot be squatted).
Every server runs in its own Docker container (bench/real/sandbox.Dockerfile) with dummy
credentials: it lists its real tools, and calls that need a real account fail authentication.

`config` reads an index report (`toollens index` output) and learns two fixes per failed
server: missing environment variables (given dummy values) and uvx executable names. It is
meant to be re-run until the catalog stops growing.
"""

import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).parent
IMAGE = "toollens-sandbox:1"

NPM = [
    ("@modelcontextprotocol/server-filesystem", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-memory", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-everything", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-sequential-thinking", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-github", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-gitlab", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-slack", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-google-maps", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-brave-search", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-puppeteer", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-postgres", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-redis", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-gdrive", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-aws-kb-retrieval", "modelcontextprotocol"),
    ("@modelcontextprotocol/server-everart", "modelcontextprotocol"),
    ("@playwright/mcp", "microsoft"),
    ("@azure/mcp", "microsoft"),
    ("@azure-devops/mcp", "microsoft"),
    ("@microsoft/clarity-mcp-server", "microsoft"),
    ("@upstash/context7-mcp", "upstash"),
    ("@upstash/mcp-server", "upstash"),
    ("@notionhq/notion-mcp-server", "makenotion"),
    ("@stripe/mcp", "stripe"),
    ("@supabase/mcp-server-supabase", "supabase"),
    ("@sentry/mcp-server", "getsentry"),
    ("@neondatabase/mcp-server-neon", "neondatabase"),
    ("@heroku/mcp-server", "heroku"),
    ("@cloudflare/mcp-server-cloudflare", "cloudflare"),
    ("firecrawl-mcp", "firecrawl"),
    ("exa-mcp-server", "exa-labs"),
    ("tavily-mcp", "tavily-ai"),
    ("@browserbasehq/mcp", "browserbase"),
    ("@elastic/mcp-server-elasticsearch", "elastic"),
    ("mongodb-mcp-server", "mongodb-js"),
    ("@hubspot/mcp-server", "hubspot"),
    ("@paypal/mcp", "paypal"),
    ("@twilio-alpha/mcp", "twilio-labs"),
    ("@shopify/dev-mcp", "shopify"),
    ("@circleci/mcp-server-circleci", "circleci-public"),
    ("@launchdarkly/mcp-server", "launchdarkly"),
    ("@apify/actors-mcp-server", "apify"),
    ("@auth0/auth0-mcp-server", "auth0"),
    ("@postman/postman-mcp-server", "postmanlabs"),
    ("@pinecone-database/mcp", "pinecone-io"),
    ("@netlify/mcp", "netlify"),
    ("@mapbox/mcp-server", "mapbox"),
    ("@google-cloud/gcloud-mcp", "googleapis"),
    ("@dynatrace-oss/dynatrace-mcp-server", "dynatrace-oss"),
    ("@browserstack/mcp-server", "browserstack"),
    ("@contentful/mcp-server", "contentful"),
    ("@sanity/mcp-server", "sanity-io"),
    ("@digitalocean/mcp", "digitalocean-labs"),
    ("@brightdata/mcp", "brightdata"),
    ("@e2b/mcp-server", "e2b-dev"),
    ("@perplexity-ai/mcp-server", "perplexityai"),
    ("@tomtom-org/tomtom-mcp", "tomtom-international"),
    ("@growthbook/mcp", "growthbook"),
    ("@arizeai/phoenix-mcp", "arize-ai"),
    ("@bitwarden/mcp-server", "bitwarden"),
    ("@chargebee/mcp", "chargebee"),
    ("@mailgun/mcp-server", "mailgun"),
    ("hostinger-api-mcp", "hostinger"),
    ("@railway/mcp-server", "railwayapp"),
    ("@wix/mcp", "wix"),
    ("@influxdata/influxdb3-mcp-server", "influxdata"),
    ("@posthog/mcp", "posthog"),
]

PYPI = [
    ("mcp-server-fetch", "modelcontextprotocol"),
    ("mcp-server-git", "modelcontextprotocol"),
    ("mcp-server-time", "modelcontextprotocol"),
    ("chroma-mcp", "chroma-core"),
    ("mcp-clickhouse", "clickhouse"),
    ("snowflake-labs-mcp", "snowflake-labs"),
    ("dbt-mcp", "dbt-labs"),
    ("redis-mcp-server", "redis"),
    ("kagimcp", "kagisearch"),
    ("semgrep-mcp", "semgrep"),
    ("logfire-mcp", "pydantic"),
    ("analytics-mcp", "googleanalytics"),
    ("opik-mcp", "comet-ml"),
    ("mcp-server-starrocks", "starrocks"),
] + [(f"awslabs.{n}", "awslabs") for n in [
    "aws-documentation-mcp-server", "aws-api-mcp-server", "cdk-mcp-server", "cfn-mcp-server",
    "eks-mcp-server", "ecs-mcp-server", "lambda-tool-mcp-server", "dynamodb-mcp-server",
    "aws-pricing-mcp-server", "cloudwatch-mcp-server", "cloudtrail-mcp-server", "iam-mcp-server",
    "s3-tables-mcp-server", "aws-diagram-mcp-server", "core-mcp-server", "bedrock-kb-retrieval-mcp-server",
    "nova-canvas-mcp-server", "frontend-mcp-server", "git-repo-research-mcp-server",
    "code-doc-gen-mcp-server", "aws-serverless-mcp-server", "stepfunctions-tool-mcp-server",
    "amazon-sns-sqs-mcp-server", "mysql-mcp-server", "postgres-mcp-server", "aurora-dsql-mcp-server",
    "documentdb-mcp-server", "elasticache-mcp-server", "memcached-mcp-server", "valkey-mcp-server",
    "amazon-neptune-mcp-server", "timestream-for-influxdb-mcp-server", "redshift-mcp-server",
    "prometheus-mcp-server", "cost-explorer-mcp-server", "billing-cost-management-mcp-server",
    "syntheticdata-mcp-server", "openapi-mcp-server", "aws-healthomics-mcp-server",
    "healthlake-mcp-server", "sagemaker-ai-mcp-server", "amazon-mq-mcp-server",
    "amazon-keyspaces-mcp-server", "aws-appsync-mcp-server", "aws-location-mcp-server",
    "aws-msk-mcp-server", "ccapi-mcp-server", "cloudwatch-appsignals-mcp-server",
    "aws-dataprocessing-mcp-server", "aws-iot-sitewise-mcp-server", "amazon-bedrock-agentcore-mcp-server",
    "aws-knowledge-mcp-server", "finch-mcp-server", "aws-bedrock-data-automation-mcp-server",
    "amazon-kendra-index-mcp-server", "amazon-qindex-mcp-server", "amazon-qbusiness-anonymous-mcp-server",
]]

# Launch arguments known from each server's README; everything else is learned from failures.
OVERRIDES = {
    "@modelcontextprotocol/server-filesystem": {"args": ["/workspace"]},
    "mcp-server-git": {"args": ["--repository", "/workspace/repo"]},
    "@azure/mcp": {"args": ["server", "start"]},
    "@stripe/mcp": {"args": ["--tools=all", "--api-key=sk_test_dummy"]},
    "@playwright/mcp": {"args": ["--headless"]},
    "@modelcontextprotocol/server-postgres": {"args": ["postgresql://localhost:1/dummy"]},
    "@modelcontextprotocol/server-redis": {"args": ["redis://localhost:1"]},
    "@cloudflare/mcp-server-cloudflare": {"args": ["run", "dummy-account-id"]},
    "@upstash/mcp-server": {"args": ["run", "dummy@example.com", "dummy-api-key"]},
    "@azure-devops/mcp": {"args": ["dummy-org"]},
    "@paypal/mcp": {"args": ["--tools=all"], "env": {"PAYPAL_ACCESS_TOKEN": "dummy", "PAYPAL_ENVIRONMENT": "SANDBOX"}},
    "@sentry/mcp-server": {"args": ["--access-token=dummy-token"]},
    "@twilio-alpha/mcp": {"args": ["AC" + "a" * 32 + "/SK" + "a" * 32 + ":dummysecret"]},
    "@launchdarkly/mcp-server": {"args": ["start", "--api-key", "api-dummy"]},
    "@auth0/auth0-mcp-server": {"args": ["run"]},
    "@brightdata/mcp": {"env": {"API_TOKEN": "dummy-not-a-real-credential"}},
    "@hubspot/mcp-server": {"env": {"PRIVATE_APP_ACCESS_TOKEN": "dummy-not-a-real-credential"}},
    "@browserstack/mcp-server": {"env": {"BROWSERSTACK_USERNAME": "dummy", "BROWSERSTACK_ACCESS_KEY": "dummy"}},
    "@influxdata/influxdb3-mcp-server": {"env": {"INFLUX_DB_INSTANCE_URL": "http://localhost:8181",
                                                 "INFLUX_DB_TOKEN": "dummy-not-a-real-credential"}},
    "@elastic/mcp-server-elasticsearch": {"env": {"ES_URL": "http://localhost:9200", "ES_API_KEY": "dummy"}},
    # Uses the asyncore loop removed in Python 3.12.
    "awslabs.amazon-keyspaces-mcp-server": {"python": "3.11"},
    "awslabs.amazon-qindex-mcp-server": {"with": ["mypy-boto3-qbusiness"]},
    "awslabs.openapi-mcp-server": {"env": {"API_NAME": "petstore",
                                           "API_SPEC_URL": "https://petstore3.swagger.io/api/v3/openapi.json",
                                           "API_BASE_URL": "https://petstore3.swagger.io/api/v3"}},
}
# Logging noise off for the AWS Labs servers; a region so boto3 can construct clients.
AWS_ENV = {"AWS_REGION": "us-east-1", "AWS_DEFAULT_REGION": "us-east-1", "FASTMCP_LOG_LEVEL": "ERROR"}


def fetch(url):
    for attempt in range(6):
        try:
            with urllib.request.urlopen(url, timeout=30) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            if e.code == 404:
                return None
            time.sleep(2 ** attempt)
        except OSError:
            time.sleep(2 ** attempt)
    raise RuntimeError(f"giving up on {url}")


def gh_orgs(urls):
    return {m.group(1).lower() for u in urls if u for m in [re.search(r"github\.com[/:]([^/]+)/", u)] if m}


def verify_one(registry, package, org):
    if registry == "npm":
        d = fetch("https://registry.npmjs.org/" + urllib.parse.quote(package, safe="@") + "/latest")
        if d is None:
            return None
        repo = d.get("repository")
        url = repo.get("url") if isinstance(repo, dict) else repo
        if org in gh_orgs([url]):
            how = "repository in vendor GitHub org"
        elif package.startswith("@") and url is None:
            how = "published under vendor npm scope"
        else:
            return None
        return {"registry": "npm", "package": package, "version": d["version"], "repo": url, "verified_by": how,
                "description": d.get("description", "")}
    d = fetch(f"https://pypi.org/pypi/{package}/json")
    if d is None:
        return None
    info = d["info"]
    urls = list((info.get("project_urls") or {}).values()) + [info.get("home_page")]
    if org not in gh_orgs(urls):
        return None
    return {"registry": "pypi", "package": package, "version": info["version"],
            "repo": next(u for u in urls if u and "github.com" in u), "verified_by": "repository in vendor GitHub org",
            "description": info.get("summary", "")}


NAMES = {
    "@shopify/dev-mcp": "shopify-dev", "@apify/actors-mcp-server": "apify", "@twilio-alpha/mcp": "twilio",
    "@arizeai/phoenix-mcp": "arize-phoenix", "@browserbasehq/mcp": "browserbase", "redis-mcp-server": "redis-cloud",
    "snowflake-labs-mcp": "snowflake", "@pinecone-database/mcp": "pinecone", "@perplexity-ai/mcp-server": "perplexity",
}


def slug(package):
    if package in NAMES:
        return NAMES[package]
    name = package.split("/")[-1].removeprefix("awslabs.")
    for noise in ("mcp-server-", "-mcp-server", "server-", "-mcp", "mcp-"):
        name = name.replace(noise, "-")
    name = re.sub(r"mcp$", "", name).strip("-") or package.split("/")[0].lstrip("@")
    if package.startswith("awslabs.") and not name.startswith(("aws", "amazon")):
        name = "aws-" + name
    if name in ("", "server"):
        name = package.split("/")[0].lstrip("@")
    return re.sub(r"[^a-z0-9-]", "-", name.lower())[:28].strip("-")


def dummy(var):
    v = var.upper()
    if "URL" in v or "URI" in v or "HOST" in v or "ENDPOINT" in v:
        return "http://localhost:1"
    if "EMAIL" in v:
        return "dummy@example.com"
    if "PORT" in v:
        return "1"
    if "REGION" in v:
        return "us-east-1"
    return "dummy-not-a-real-credential"


ENV_HINT = re.compile(r"environment|env var|\benv\b|required|missing|not set|must be set|must provide|is not defined", re.I)
ENV_NAME = re.compile(r"\b([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)\b")
NOT_ENV = {"MCP_SERVER", "JSON_RPC", "HTTP_ERROR", "API_KEY", "ERR_MODULE_NOT_FOUND", "ERR_INVALID_ARG_TYPE", "UTF_8"}
UVX_EXE = re.compile(r"executables? (?:are|is) available:?[\s/]*-\s*([A-Za-z0-9_.\-]+)")
# Servers written against MCP Python SDK 1.x that do not cap it; the SDK's own error says to pin.
MCP_V1 = "pin 'mcp<2'"


def learn(report, learned):
    """Adds dummy env vars and uvx executable names suggested by each failure's stderr."""
    for f in report.get("failed", []):
        err, entry = f["error"], learned.setdefault(f["server"], {})
        if ENV_HINT.search(err):
            for var in ENV_NAME.findall(err):
                if var not in NOT_ENV and not var.startswith("ERR_"):
                    entry.setdefault("env", {})[var] = dummy(var)
        m = UVX_EXE.search(err)
        if m:
            entry["exe"] = m.group(1)
        if MCP_V1 in err:
            entry["with"] = sorted(set(entry.get("with", [])) | {"mcp<2"})
    return learned


def config(report_path=None):
    servers = json.loads((HERE / "servers.json").read_text(encoding="utf-8"))
    learned_path = HERE / "learned.json"
    learned = json.loads(learned_path.read_text(encoding="utf-8")) if learned_path.exists() else {}
    if report_path:
        learn(json.loads(Path(report_path).read_text(encoding="utf-8")), learned)
        learned_path.write_text(json.dumps(learned, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    mcp, seen = {}, set()
    for s in servers:
        name = slug(s["package"])
        while name in seen:
            name += "-2"
        seen.add(name)
        o, l = OVERRIDES.get(s["package"], {}), learned.get(name, {})
        env = {**(AWS_ENV if s["package"].startswith("awslabs.") else {}), **o.get("env", {}), **l.get("env", {})}
        docker = ["run", "-i", "--rm", "--init", "--label", "toollens-real=1", "--memory", "768m", "--cpus", "1",
                  "--pids-limit", "512", "-v", "toollens-cache:/cache"]
        for k, v in sorted(env.items()):
            docker += ["-e", f"{k}={v}"]
        docker.append(IMAGE)
        if s["registry"] == "npm":
            launch = ["npx", "-y", f"{s['package']}@{s['version']}"]
        else:
            launch = ["uvx"]
            if "python" in o:
                launch += ["--python", o["python"]]
            for w in sorted(set(o.get("with", [])) | set(l.get("with", []))):
                launch += ["--with", w]
            if "exe" in l:
                launch += ["--from", f"{s['package']}=={s['version']}", l["exe"]]
            else:
                launch.append(f"{s['package']}@{s['version']}")
        mcp[name] = {"command": "docker", "args": docker + launch + o.get("args", [])}
    cfg = {
        "mcpServers": mcp,
        "lazy": True,
        "maxLive": 6,
        # Dummy credentials mean no call can change a real account; destructive tools stay denied anyway.
        "policy": {"default": "allow", "deny": [], "allow": [], "denyDestructive": True},
    }
    (HERE / "gateway.json").write_text(json.dumps(cfg, indent=1) + "\n", encoding="utf-8")
    print(f"wrote gateway.json with {len(mcp)} servers ({len(learned)} with learned fixes)")


def finalize(report_path):
    """catalog.json: gateway.json restricted to servers that indexed, so every arm and every run
    sees the same catalog and nothing is retried. Configs are unchanged, so the index cache hits."""
    report = json.loads(Path(report_path).read_text(encoding="utf-8"))
    cfg = json.loads((HERE / "gateway.json").read_text(encoding="utf-8"))
    ok = set(report["servers"])
    cfg["mcpServers"] = {k: v for k, v in cfg["mcpServers"].items() if k in ok}
    (HERE / "catalog.json").write_text(json.dumps(cfg, indent=1) + "\n", encoding="utf-8")
    failed = sorted(f["server"] for f in report["failed"])
    print(f"catalog.json: {len(cfg['mcpServers'])} servers, {report['tools']} tools; left out: {', '.join(failed)}")


def verify():
    with ThreadPoolExecutor(12) as ex:
        rows = list(ex.map(lambda a: verify_one("npm", *a), NPM)) + list(ex.map(lambda a: verify_one("pypi", *a), PYPI))
    kept = [r for r in rows if r]
    (HERE / "servers.json").write_text(json.dumps(kept, indent=1) + "\n", encoding="utf-8")
    print(f"verified {len(kept)} of {len(NPM) + len(PYPI)} candidates -> servers.json")


if __name__ == "__main__":
    {
        "verify": verify,
        "config": lambda: config(sys.argv[2] if len(sys.argv) > 2 else None),
        "finalize": lambda: finalize(sys.argv[2]),
    }[sys.argv[1]]()
