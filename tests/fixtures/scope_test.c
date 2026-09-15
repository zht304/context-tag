/* Scope analysis test */
#include <stdio.h>

/* Global scope */
int global_var = 100;

/* Function scope */
void test_shadowing() {
    int global_var = 200;  /* Shadows global */
    printf("Inner: %d\n", global_var);

    /* Block scope */
    {
        int global_var = 300;  /* Shadows function-local */
        int block_only = 400;
        printf("Block: %d, %d\n", global_var, block_only);
    }

    printf("Outer: %d\n", global_var);
}

/* Nested function-like behavior with static */
static int static_helper(int x) {
    static int call_count = 0;
    call_count++;
    return x * call_count;
}

/* Parameter scope */
void param_test(int a, int b, int c) {
    int result = a + b + c;
    (void)result;
}

/* Static vs extern */
static int file_local = 0;

void update_local() {
    file_local++;
}

int get_local() {
    return file_local;
}

/* Typedef scope */
typedef struct {
    int data;
    struct Node *next;
} Node;

Node *create_node(int val) {
    Node *n = (Node *)malloc(sizeof(Node));
    n->data = val;
    n->next = NULL;
    return n;
}

void free_node(Node *n) {
    free(n);
}
