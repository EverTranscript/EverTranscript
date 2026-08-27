# Fallback is cloud→local only, never local→cloud

On Backend failure (e.g., cloud Summary erroring mid-generation) a feature may auto-switch from cloud to local, never the reverse: silently switching local→cloud would betray an explicit privacy choice. A live active-backend indicator is required whenever Fallback occurs, and Strict Mode disables Fallback entirely ("never auto-switch; tell me on failure").
