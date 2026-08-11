class SchedulerError(RuntimeError):
    """Base error for rejected scheduler operations."""


class InvalidTransition(SchedulerError):
    pass


class StaleAuthority(SchedulerError):
    pass


class NotFound(SchedulerError):
    pass


class ConfigurationError(SchedulerError):
    pass


class AdapterError(SchedulerError):
    pass
