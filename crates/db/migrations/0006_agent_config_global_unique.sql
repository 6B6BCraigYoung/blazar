DELETE FROM agent_configs WHERE rowid IN (
    SELECT rowid FROM (
        SELECT rowid,
               ROW_NUMBER() OVER (PARTITION BY node_id, agent_id
                                  ORDER BY updated_at DESC, rowid DESC) AS rn
        FROM agent_configs
    ) WHERE rn > 1
);

CREATE UNIQUE INDEX uq_agent_configs_global
    ON agent_configs(agent_id) WHERE node_id IS NULL;
