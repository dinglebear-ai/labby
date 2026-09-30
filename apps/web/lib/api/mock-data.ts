import type {
  Gateway,
  ExposurePolicy,
  ExposurePolicyPreview,
  ReloadGatewayResult,
  ServiceAction,
  ServiceConfig,
  SupportedService,
  TestGatewayResult,
} from '@/lib/types/gateway'

export const mockGateways: Gateway[] = [
  {
    id: 'gw-1',
    name: 'filesystem-server',
    transport: 'stdio',
    config: {
      command: 'npx',
      args: ['-y', '@modelcontextprotocol/server-filesystem', '/home/user/documents'],
      proxy_resources: true,
      expose_tools: ['read_file', 'write_file', 'list_directory'],
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 8,
      exposed_tool_count: 3,
      discovered_resource_count: 1,
      exposed_resource_count: 1,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools: [
        { name: 'read_file', description: 'Read contents of a file', exposed: true, matched_by: 'read_file' },
        { name: 'write_file', description: 'Write contents to a file', exposed: true, matched_by: 'write_file' },
        { name: 'list_directory', description: 'List directory contents', exposed: true, matched_by: 'list_directory' },
        { name: 'create_directory', description: 'Create a new directory', exposed: false, matched_by: null },
        { name: 'delete_file', description: 'Delete a file', exposed: false, matched_by: null },
        { name: 'move_file', description: 'Move or rename a file', exposed: false, matched_by: null },
        { name: 'copy_file', description: 'Copy a file', exposed: false, matched_by: null },
        { name: 'get_file_info', description: 'Get file metadata', exposed: false, matched_by: null },
      ],
      resources: [
        { name: 'file', uri: 'file://{path}', description: 'Access file contents' },
      ],
      prompts: [],
    },
    warnings: [],
    created_at: '2024-01-15T10:30:00Z',
    updated_at: '2024-01-20T14:22:00Z',
  },
  {
    id: 'gw-2',
    name: 'github-server',
    transport: 'http',
    config: {
      url: 'http://localhost:3001/mcp',
      bearer_token_env: 'GITHUB_TOKEN',
      proxy_resources: true,
      proxy_skills: true,
      expose_skills: ['review-*'],
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 12,
      exposed_tool_count: 12,
      discovered_resource_count: 3,
      exposed_resource_count: 3,
      discovered_prompt_count: 2,
      exposed_prompt_count: 2,
      discovered_skill_count: 2,
      exposed_skill_count: 1,
      supports_skills: true,
    },
    discovery: {
      tools: [
        { name: 'create_issue', description: 'Create a new GitHub issue', exposed: true, matched_by: '*' },
        { name: 'list_issues', description: 'List repository issues', exposed: true, matched_by: '*' },
        { name: 'get_issue', description: 'Get issue details', exposed: true, matched_by: '*' },
        { name: 'update_issue', description: 'Update an issue', exposed: true, matched_by: '*' },
        { name: 'create_pull_request', description: 'Create a pull request', exposed: true, matched_by: '*' },
        { name: 'list_pull_requests', description: 'List pull requests', exposed: true, matched_by: '*' },
        { name: 'get_pull_request', description: 'Get PR details', exposed: true, matched_by: '*' },
        { name: 'merge_pull_request', description: 'Merge a pull request', exposed: true, matched_by: '*' },
        { name: 'list_repos', description: 'List repositories', exposed: true, matched_by: '*' },
        { name: 'get_repo', description: 'Get repository details', exposed: true, matched_by: '*' },
        { name: 'search_code', description: 'Search code in repositories', exposed: true, matched_by: '*' },
        { name: 'get_file_contents', description: 'Get file contents from repo', exposed: true, matched_by: '*' },
      ],
      resources: [
        { name: 'repository', uri: 'github://repo/{owner}/{name}', description: 'Repository resource' },
        { name: 'issue', uri: 'github://issue/{owner}/{repo}/{number}', description: 'Issue resource' },
        { name: 'pull_request', uri: 'github://pr/{owner}/{repo}/{number}', description: 'Pull request resource' },
      ],
      prompts: [
        { name: 'create_issue_from_description', description: 'Create an issue from natural language', arguments: [{ name: 'description', required: true }] },
        { name: 'summarize_pr', description: 'Summarize a pull request', arguments: [{ name: 'pr_url', required: true }] },
      ],
    },
    warnings: [],
    created_at: '2024-01-10T08:00:00Z',
    updated_at: '2024-01-22T09:15:00Z',
  },
  {
    id: 'gw-3',
    name: 'slack-server',
    transport: 'http',
    config: {
      url: 'http://localhost:3002/mcp',
      bearer_token_env: 'SLACK_BOT_TOKEN',
      proxy_resources: false,
      proxy_skills: false,
      expose_tools: ['send_message', 'list_channels', 'search_*'],
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 15,
      exposed_tool_count: 5,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 1,
      exposed_prompt_count: 1,
      discovered_skill_count: 0,
      exposed_skill_count: 0,
      supports_skills: true,
    },
    discovery: {
      tools: [
        { name: 'send_message', description: 'Send a message to a channel', exposed: true, matched_by: 'send_message' },
        { name: 'list_channels', description: 'List available channels', exposed: true, matched_by: 'list_channels' },
        { name: 'search_messages', description: 'Search messages', exposed: true, matched_by: 'search_*' },
        { name: 'search_files', description: 'Search files', exposed: true, matched_by: 'search_*' },
        { name: 'search_users', description: 'Search users', exposed: true, matched_by: 'search_*' },
        { name: 'upload_file', description: 'Upload a file', exposed: false, matched_by: null },
        { name: 'delete_message', description: 'Delete a message', exposed: false, matched_by: null },
        { name: 'create_channel', description: 'Create a new channel', exposed: false, matched_by: null },
        { name: 'archive_channel', description: 'Archive a channel', exposed: false, matched_by: null },
        { name: 'invite_user', description: 'Invite user to channel', exposed: false, matched_by: null },
        { name: 'kick_user', description: 'Remove user from channel', exposed: false, matched_by: null },
        { name: 'set_topic', description: 'Set channel topic', exposed: false, matched_by: null },
        { name: 'pin_message', description: 'Pin a message', exposed: false, matched_by: null },
        { name: 'add_reaction', description: 'Add reaction to message', exposed: false, matched_by: null },
        { name: 'get_user_info', description: 'Get user information', exposed: false, matched_by: null },
      ],
      resources: [],
      prompts: [
        { name: 'daily_summary', description: 'Generate daily channel summary', arguments: [{ name: 'channel', required: true }] },
      ],
    },
    warnings: [
      { code: 'UNMATCHED_PATTERN', message: 'Pattern "read_*" does not match any discovered tools', timestamp: '2024-01-22T08:00:00Z' },
    ],
    created_at: '2024-01-12T11:00:00Z',
    updated_at: '2024-01-22T08:00:00Z',
  },
  {
    id: 'gw-4',
    name: 'database-server',
    transport: 'stdio',
    config: {
      command: '/usr/local/bin/mcp-postgres',
      args: ['--connection-string', 'postgresql://localhost/mydb'],
      proxy_resources: true,
    },
    status: {
      healthy: false,
      connected: false,
      last_error: 'Connection refused: Unable to connect to PostgreSQL server at localhost:5432',
      discovered_tool_count: 0,
      exposed_tool_count: 0,
      discovered_resource_count: 0,
      exposed_resource_count: 0,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools: [],
      resources: [],
      prompts: [],
    },
    warnings: [
      { code: 'CONNECTION_FAILED', message: 'Failed to establish connection to upstream server', timestamp: '2024-01-22T10:30:00Z' },
      { code: 'DISCOVERY_FAILED', message: 'Could not discover MCP capabilities due to connection failure', timestamp: '2024-01-22T10:30:00Z' },
    ],
    created_at: '2024-01-18T16:00:00Z',
    updated_at: '2024-01-22T10:30:00Z',
  },
  {
    id: 'gw-5',
    name: 'memory-server',
    transport: 'stdio',
    config: {
      command: 'npx',
      args: ['-y', '@modelcontextprotocol/server-memory'],
      proxy_resources: true,
    },
    status: {
      healthy: true,
      connected: true,
      discovered_tool_count: 4,
      exposed_tool_count: 4,
      discovered_resource_count: 1,
      exposed_resource_count: 1,
      discovered_prompt_count: 0,
      exposed_prompt_count: 0,
    },
    discovery: {
      tools: [
        { name: 'store_memory', description: 'Store a memory with key', exposed: true, matched_by: '*' },
        { name: 'retrieve_memory', description: 'Retrieve a stored memory', exposed: true, matched_by: '*' },
        { name: 'list_memories', description: 'List all stored memories', exposed: true, matched_by: '*' },
        { name: 'delete_memory', description: 'Delete a stored memory', exposed: true, matched_by: '*' },
      ],
      resources: [
        { name: 'memory', uri: 'memory://{key}', description: 'Access stored memory by key' },
      ],
      prompts: [],
    },
    warnings: [],
    created_at: '2024-01-20T09:00:00Z',
    updated_at: '2024-01-21T15:00:00Z',
  },
]

export const mockExposurePolicy: ExposurePolicy = {
  mode: 'allowlist',
  patterns: ['read_file', 'write_file', 'list_directory'],
}

export const mockExposurePolicyPreview: ExposurePolicyPreview = {
  matched_tools: [
    { name: 'read_file', matched_by: 'read_file' },
    { name: 'write_file', matched_by: 'write_file' },
    { name: 'list_directory', matched_by: 'list_directory' },
  ],
  unmatched_patterns: [],
  filtered_tools: ['create_directory', 'delete_file', 'move_file', 'copy_file', 'get_file_info'],
  exposed_count: 3,
  filtered_count: 5,
}

export const mockTestResult: TestGatewayResult = {
  success: true,
  message: 'Gateway connection successful',
  latency_ms: 42,
  discovered_tools: 8,
  discovered_resources: 1,
  discovered_prompts: 0,
}

export const mockReloadResult: ReloadGatewayResult = {
  success: true,
  message: 'Gateway reloaded successfully',
  previous_tool_count: 8,
  new_tool_count: 8,
}

export const mockSupportedServices: SupportedService[] = [
  {
    key: 'filesystem',
    display_name: 'Filesystem',
    category: 'Documents',
    description: 'Local filesystem access for reading and writing project files.',
    required_env: [
      {
        name: 'FILESYSTEM_URL',
        description: 'Base URL for the filesystem adapter',
        example: 'http://localhost:8787',
        secret: false,
      },
    ],
    optional_env: [
      {
        name: 'FILESYSTEM_TOKEN',
        description: 'Optional bearer token for the filesystem adapter',
        example: 'filesystem-dev-token',
        secret: true,
      },
    ],
    default_port: 8787,
  },
  {
    key: 'github',
    display_name: 'GitHub',
    category: 'Developer Tools',
    description: 'Repository search, issue management, and pull request workflows.',
    required_env: [
      {
        name: 'GITHUB_URL',
        description: 'Base URL for the GitHub MCP adapter',
        example: 'https://github.example.com/mcp',
        secret: false,
      },
      {
        name: 'GITHUB_TOKEN',
        description: 'GitHub personal access token',
        example: 'ghp_xxxxxxxxxxxx',
        secret: true,
      },
    ],
    optional_env: [],
    default_port: 443,
  },
  {
    key: 'slack',
    display_name: 'Slack',
    category: 'Notifications',
    description: 'Channel search, posting, and workspace lookup tools.',
    required_env: [
      {
        name: 'SLACK_URL',
        description: 'Base URL for the Slack MCP adapter',
        example: 'https://slack.example.com/mcp',
        secret: false,
      },
      {
        name: 'SLACK_BOT_TOKEN',
        description: 'Slack bot token used for API access',
        example: 'xoxb-xxxxxxxxxxxx',
        secret: true,
      },
    ],
    optional_env: [],
    default_port: 443,
  },
  {
    key: 'memory',
    display_name: 'Memory',
    category: 'Notes',
    description: 'A lightweight key-value memory service for assistants.',
    required_env: [
      {
        name: 'MEMORY_URL',
        description: 'Base URL for the memory MCP adapter',
        example: 'http://localhost:8788',
        secret: false,
      },
    ],
    optional_env: [],
    default_port: 8788,
  },
]

export const mockServiceConfigs: Record<string, ServiceConfig> = {
  filesystem: {
    service: 'filesystem',
    configured: true,
    fields: [
      { name: 'FILESYSTEM_URL', present: true, secret: false, value_preview: 'http://localhost:8787' },
      { name: 'FILESYSTEM_TOKEN', present: false, secret: true, value_preview: null },
    ],
  },
  github: {
    service: 'github',
    configured: true,
    fields: [
      { name: 'GITHUB_URL', present: true, secret: false, value_preview: 'https://github.example.com/mcp' },
      { name: 'GITHUB_TOKEN', present: true, secret: true, value_preview: '••••••••' },
    ],
  },
  slack: {
    service: 'slack',
    configured: true,
    fields: [
      { name: 'SLACK_URL', present: true, secret: false, value_preview: 'https://slack.example.com/mcp' },
      { name: 'SLACK_BOT_TOKEN', present: true, secret: true, value_preview: '••••••••' },
    ],
  },
  memory: {
    service: 'memory',
    configured: false,
    fields: [{ name: 'MEMORY_URL', present: false, secret: false, value_preview: 'http://localhost:8788' }],
  },
}

export const mockServiceActions: Record<string, ServiceAction[]> = {
  filesystem: [
    { name: 'read_file', description: 'Read contents of a file', destructive: false },
    { name: 'write_file', description: 'Write a file to disk', destructive: true },
    { name: 'list_directory', description: 'List directory contents', destructive: false },
  ],
  github: [
    { name: 'list_issues', description: 'List repository issues', destructive: false },
    { name: 'create_issue', description: 'Create a GitHub issue', destructive: true },
    { name: 'merge_pull_request', description: 'Merge a pull request', destructive: true },
  ],
  slack: [
    { name: 'send_message', description: 'Send a Slack message', destructive: true },
    { name: 'list_channels', description: 'List Slack channels', destructive: false },
    { name: 'search_messages', description: 'Search Slack messages', destructive: false },
  ],
  memory: [
    { name: 'store_memory', description: 'Store a memory by key', destructive: true },
    { name: 'retrieve_memory', description: 'Retrieve a stored memory', destructive: false },
    { name: 'list_memories', description: 'List stored memories', destructive: false },
    { name: 'delete_memory', description: 'Delete a stored memory', destructive: true },
  ],
}
