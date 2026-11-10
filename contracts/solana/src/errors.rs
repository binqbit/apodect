use anchor_lang::prelude::*;

#[error_code]
pub enum ContractError {
    #[msg("Amount must be positive")]
    ZeroAmount,
    #[msg("Purpose reference must be nonzero")]
    EmptyPurpose,
    #[msg("Checked amount or counter arithmetic failed")]
    Arithmetic,
    #[msg("This exact action is not authorized for this signer")]
    Unauthorized,
    #[msg("Task has completed or was canceled")]
    TerminalTask,
    #[msg("Task lifecycle or control state does not permit this action")]
    InvalidTaskState,
    #[msg("A task may have only one live request")]
    LiveRequest,
    #[msg("The provided request does not match the task's live request")]
    WrongLiveRequest,
    #[msg("Invalid request state")]
    InvalidRequestState,
    #[msg("Provided queue head or tail does not match the Project queue")]
    WrongQueueAccount,
    #[msg("Insufficient free Project funds for the complete approved amount")]
    InsufficientProjectFunds,
    #[msg("Amount exceeds the available controlled allocation")]
    InsufficientAllocation,
    #[msg("Return exceeds this task's external outstanding principal")]
    ExcessReturn,
    #[msg("Ordinary cancel is unavailable after first accepted use")]
    AlreadyStarted,
    #[msg("Task must have been funded and release all controlled funds before finalization")]
    CannotFinalize,
    #[msg("Invoice must belong to this Project and be closed for linked funding")]
    InvalidInvoice,
    #[msg("Invoice is already closed")]
    InvoiceClosed,
    #[msg("Project asset must be a six-decimal legacy SPL mint")]
    InvalidMint,
}
